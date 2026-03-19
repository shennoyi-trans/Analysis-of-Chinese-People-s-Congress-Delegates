use anyhow::{anyhow, Context, Result};
use chrono::Local;
use csv::{ReaderBuilder, WriterBuilder};
use futures::stream::{self, StreamExt};
use plotters::prelude::*;
use regex::Regex;
use reqwest::{Client, StatusCode};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Debug, Deserialize, Clone)]
struct Config {
    provider: String,
    api_key: String,
    model: String,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default = "default_max_concurrency")]
    max_concurrency: usize,
    #[serde(default = "default_npc_term")]
    npc_term: u32,
}

fn default_max_concurrency() -> usize {
    5
}

fn default_npc_term() -> u32 {
    14
}

#[derive(Debug, Deserialize, Clone)]
struct DelegateInput {
    #[serde(alias = "姓名", alias = "代表姓名")]
    name: String,
    #[serde(alias = "省份", alias = "代表团", alias = "地区")]
    province: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Category {
    PartyGovernmentCadre,
    Entrepreneur,
    GrassrootsWorkerFarmer,
    MilitaryArmedPolice,
    OtherRepresentative,
    Unknown,
}

impl Category {
    fn as_cn(&self) -> &'static str {
        match self {
            Category::PartyGovernmentCadre => "党政干部",
            Category::Entrepreneur => "企业家",
            Category::GrassrootsWorkerFarmer => "工农基层代表",
            Category::MilitaryArmedPolice => "解放军和武警系统代表",
            Category::OtherRepresentative => "其他各行业各领域代表",
            Category::Unknown => "未知",
        }
    }

    fn all_cn() -> Vec<&'static str> {
        vec![
            "党政干部",
            "企业家",
            "工农基层代表",
            "解放军和武警系统代表",
            "其他各行业各领域代表",
            "未知",
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RecordStatus {
    Success,
    InsufficientInfo,
    LlmError,
}

impl RecordStatus {
    fn as_str(&self) -> &'static str {
        match self {
            RecordStatus::Success => "success",
            RecordStatus::InsufficientInfo => "insufficient_info",
            RecordStatus::LlmError => "llm_error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmResult {
    name: String,
    province: String,
    primary_category: Category,
    is_political_business_combo: bool,
    combo_brief: String,
    reason: String,
    confidence: Option<f64>,
    sources: Vec<String>,
    status: RecordStatus,
}

#[derive(Debug, Clone, Serialize)]
struct OutputRow {
    name: String,
    province: String,
    primary_category: String,
    is_political_business_combo: bool,
    combo_brief: String,
    reason: String,
    confidence: Option<f64>,
    sources: String,
    status: String,
}

#[derive(Debug, Clone)]
enum ApiError {
    Unauthorized(String),
    Retryable(String),
    NonRetryable(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Unauthorized(msg) => write!(f, "{msg}"),
            ApiError::Retryable(msg) => write!(f, "{msg}"),
            ApiError::NonRetryable(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

type CacheMap = HashMap<String, LlmResult>;

fn exe_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("无法获取当前可执行文件路径")?;
    let dir = exe.parent().ok_or_else(|| anyhow!("无法获取程序所在目录"))?;
    Ok(dir.to_path_buf())
}

fn output_dir(base_dir: &Path) -> Result<PathBuf> {
    let out = base_dir.join("output");
    if !out.exists() {
        fs::create_dir_all(&out)
            .with_context(|| format!("创建输出目录失败: {}", out.display()))?;
    }
    Ok(out)
}

fn cache_file_path(out_dir: &Path) -> PathBuf {
    out_dir.join("cache.json")
}

fn delegates_csv_path(out_dir: &Path) -> PathBuf {
    out_dir.join("delegates.csv")
}

fn cache_key(name: &str, province: &str) -> String {
    format!("{}||{}", name.trim(), province.trim())
}

fn should_cache(result: &LlmResult) -> bool {
    matches!(
        result.status,
        RecordStatus::Success | RecordStatus::InsufficientInfo
    )
}

fn load_cache(out_dir: &Path) -> Result<CacheMap> {
    let path = cache_file_path(out_dir);
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let text = fs::read_to_string(&path)
        .with_context(|| format!("读取缓存失败: {}", path.display()))?;

    if text.trim().is_empty() {
        return Ok(HashMap::new());
    }

    let cache: CacheMap = serde_json::from_str(&text)
        .with_context(|| format!("解析缓存失败: {}", path.display()))?;
    Ok(cache)
}

fn save_cache(out_dir: &Path, cache: &CacheMap) -> Result<()> {
    let path = cache_file_path(out_dir);
    let text = serde_json::to_string_pretty(cache)?;
    fs::write(&path, text).with_context(|| format!("写入缓存失败: {}", path.display()))?;
    Ok(())
}

fn read_config(dir: &Path) -> Result<Config> {
    let path = dir.join("config.json");
    let text = fs::read_to_string(&path)
        .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
    let cfg: Config = serde_json::from_str(&text).context("config.json 解析失败")?;
    Ok(cfg)
}

fn read_delegates(csv_path: &Path) -> Result<Vec<DelegateInput>> {
    let mut rdr = ReaderBuilder::new()
        .flexible(true)
        .from_path(csv_path)
        .with_context(|| format!("打开 CSV 失败: {}", csv_path.display()))?;

    let mut rows = Vec::new();

    for result in rdr.deserialize() {
        let mut record: DelegateInput = result.with_context(|| {
            format!(
                "CSV 解析失败，请确认有 name/province 或 姓名/省份 两列: {}",
                csv_path.display()
            )
        })?;

        record.name = record.name.trim().to_string();
        record.province = record.province.trim().to_string();

        if record.name.is_empty() && record.province.is_empty() {
            continue;
        }
        if record.name.is_empty() {
            continue;
        }

        rows.push(record);
    }

    Ok(rows)
}

fn build_prompt(name: &str, province: &str) -> String {
    format!(
r#"输入信息：
- 人名：{name}
- 省份/代表团：{province}
"#)
}

fn normalize_category(s: &str) -> Category {
    match s.trim() {
        "党政干部" => Category::PartyGovernmentCadre,
        "企业家" => Category::Entrepreneur,
        "工农基层代表" => Category::GrassrootsWorkerFarmer,
        "解放军和武警系统代表" => Category::MilitaryArmedPolice,
        "其他各行业各领域代表" => Category::OtherRepresentative,
        _ => Category::Unknown,
    }
}

fn is_insufficient_info_result(r: &LlmResult) -> bool {
    matches!(r.primary_category, Category::Unknown)
        && r.confidence.unwrap_or(0.0) <= 0.35
        && !r.is_political_business_combo
}

async fn call_llm_once(
    client: &Client,
    cfg: &Config,
    name: &str,
    province: &str,
) -> std::result::Result<LlmResult, ApiError> {

    let system_prompt = r#"你是一个严谨的信息分类助手。
任务：
请对“人大代表”进行身份识别。你必须优先联网搜索公开资料（官方简历、政府网站、新闻报道、权威媒体等），再给出结论。

分类规则（primary_category 只能从以下六类中选一个）：
1. 党政干部
2. 企业家
3. 工农基层代表
4. 解放军和武警系统代表
5. 其他各行业各领域代表
6. 未知

判定要求：
- 若此人同时具有党政/人大/政协/政府系统身份和明显企业经营者/企业控制人/董事长/总裁等商界身份，请：
  - 仍然只选择一个“主要身份”写入 primary_category
  - 但必须将 is_political_business_combo 设为 true
  - 并在 combo_brief 中简要说明其“政商合一”情况
- 若资料不足，选择“未知”
- 尽量基于最近、明确、可核验的信息
- reason 中用简洁中文写出判断依据
- sources 给出你参考的网站标题或链接摘要，最多 5 条

输出要求：
只输出一个 JSON 对象，不要输出 markdown，不要输出代码块，不要输出多余解释。
JSON 格式必须严格如下：

{
  "name": "{name}",
  "province": "{province}",
  "primary_category": "党政干部|企业家|工农基层代表|解放军和武警系统代表|其他各行业各领域代表|未知",
  "is_political_business_combo": true,
  "combo_brief": "若非政商合一可写空字符串",
  "reason": "简要说明",
  "confidence": 0.0,
  "sources": ["来源1", "来源2"]
}

注意：
- confidence 取值 0 到 1
- 如果无法确认，请降低 confidence
- 必须保证 JSON 合法"#.to_string();

    let prompt = build_prompt(name, province);

    let base_url = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| "https://api.deepseek.com".to_string());

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let body = json!({
        "model": cfg.model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": prompt}
        ],
        "temperature": 0.1
    });

    let resp = client
        .post(&url)
        .bearer_auth(&cfg.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            let msg = format!("LLM 请求失败: {} | {}", name, e);
            if e.is_timeout() || e.is_connect() || e.is_request() {
                ApiError::Retryable(msg)
            } else {
                ApiError::NonRetryable(msg)
            }
        })?;

    let status = resp.status();

    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let msg = format!("LLM 接口返回错误 {}: {}", status, text);

        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized(msg));
        }

        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(ApiError::Retryable(msg));
        }

        return Err(ApiError::NonRetryable(msg));
    }

    let v: Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Retryable(format!("解析 LLM 响应 JSON 失败: {}", e)))?;

    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| ApiError::NonRetryable("未找到模型输出内容".to_string()))?
        .trim()
        .to_string();

    parse_llm_output(&content, name, province)
        .map_err(|e| ApiError::NonRetryable(format!("解析模型输出失败: {}", e)))
}

async fn call_llm_with_retry(
    client: &Client,
    cfg: &Config,
    name: &str,
    province: &str,
) -> std::result::Result<LlmResult, ApiError> {
    let max_attempts = 3usize;

    for attempt in 1..=max_attempts {
        match call_llm_once(client, cfg, name, province).await {
            Ok(mut r) => {
                r.status = if is_insufficient_info_result(&r) {
                    RecordStatus::InsufficientInfo
                } else {
                    RecordStatus::Success
                };
                return Ok(r);
            }
            Err(ApiError::Unauthorized(msg)) => {
                return Err(ApiError::Unauthorized(msg));
            }
            Err(ApiError::Retryable(msg)) => {
                if attempt < max_attempts {
                    let wait_secs = attempt as u64 * 2;
                    eprintln!(
                        "  !! {} 第 {}/{} 次请求失败，{} 秒后重试。原因：{}",
                        name, attempt, max_attempts, wait_secs, msg
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
                    continue;
                } else {
                    return Err(ApiError::Retryable(format!(
                        "多次重试后仍失败：{}",
                        msg
                    )));
                }
            }
            Err(ApiError::NonRetryable(msg)) => {
                return Err(ApiError::NonRetryable(msg));
            }
        }
    }

    Err(ApiError::Retryable("未知重试错误".to_string()))
}

fn strip_code_fence(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with("```") {
        let mut lines: Vec<&str> = trimmed.lines().collect();
        if !lines.is_empty() {
            lines.remove(0);
        }
        if !lines.is_empty() && lines.last() == Some(&"```") {
            lines.pop();
        }
        return lines.join("\n").trim().to_string();
    }
    trimmed.to_string()
}

fn parse_llm_output(content: &str, fallback_name: &str, fallback_province: &str) -> Result<LlmResult> {
    let cleaned = strip_code_fence(content);

    let v: Value = serde_json::from_str(&cleaned)
        .or_else(|_| {
            let start = cleaned.find('{').ok_or_else(|| anyhow!("未找到 JSON 起始"))?;
            let end = cleaned.rfind('}').ok_or_else(|| anyhow!("未找到 JSON 结束"))?;
            serde_json::from_str::<Value>(&cleaned[start..=end]).map_err(|e| anyhow!(e))
        })
        .context("模型输出不是合法 JSON")?;

    let name = v["name"].as_str().unwrap_or(fallback_name).to_string();
    let province = v["province"].as_str().unwrap_or(fallback_province).to_string();
    let category_str = v["primary_category"].as_str().unwrap_or("未知");
    let primary_category = normalize_category(category_str);
    let is_political_business_combo = v["is_political_business_combo"].as_bool().unwrap_or(false);
    let combo_brief = v["combo_brief"].as_str().unwrap_or("").to_string();
    let reason = v["reason"].as_str().unwrap_or("").to_string();
    let confidence = v["confidence"].as_f64();

    let mut sources = vec![];
    if let Some(arr) = v["sources"].as_array() {
        for item in arr {
            if let Some(s) = item.as_str() {
                sources.push(s.to_string());
            }
        }
    }

    Ok(LlmResult {
        name,
        province,
        primary_category,
        is_political_business_combo,
        combo_brief,
        reason,
        confidence,
        sources,
        status: RecordStatus::Success,
    })
}

fn write_results_csv(out_dir: &Path, results: &[LlmResult]) -> Result<()> {
    let path = out_dir.join("results.csv");
    let mut wtr = WriterBuilder::new()
        .from_path(&path)
        .with_context(|| format!("写入失败: {}", path.display()))?;

    for r in results {
        let row = OutputRow {
            name: r.name.clone(),
            province: r.province.clone(),
            primary_category: r.primary_category.as_cn().to_string(),
            is_political_business_combo: r.is_political_business_combo,
            combo_brief: r.combo_brief.clone(),
            reason: r.reason.clone(),
            confidence: r.confidence,
            sources: r.sources.join(" | "),
            status: r.status.as_str().to_string(),
        };
        wtr.serialize(row)?;
    }

    wtr.flush()?;
    Ok(())
}

fn write_results_json(out_dir: &Path, results: &[LlmResult]) -> Result<()> {
    let path = out_dir.join("results.json");
    let text = serde_json::to_string_pretty(results)?;
    fs::write(&path, text).with_context(|| format!("写入失败: {}", path.display()))?;
    Ok(())
}

fn write_summary_csv(out_dir: &Path, results: &[LlmResult]) -> Result<BTreeMap<String, usize>> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for cat in Category::all_cn() {
        counts.insert(cat.to_string(), 0);
    }
    for r in results {
        let key = r.primary_category.as_cn().to_string();
        *counts.entry(key).or_insert(0) += 1;
    }

    let path = out_dir.join("summary.csv");
    let mut wtr = WriterBuilder::new()
        .from_path(&path)
        .with_context(|| format!("写入失败: {}", path.display()))?;

    wtr.write_record(["category", "count"])?;
    for (k, v) in &counts {
        wtr.write_record([k, &v.to_string()])?;
    }
    wtr.flush()?;

    Ok(counts)
}

fn write_combo_csv(out_dir: &Path, results: &[LlmResult]) -> Result<()> {
    let path = out_dir.join("political_business_combo.csv");
    let mut wtr = WriterBuilder::new()
        .from_path(&path)
        .with_context(|| format!("写入失败: {}", path.display()))?;

    wtr.write_record(["name", "province", "primary_category", "combo_brief", "reason", "status"])?;
    for r in results.iter().filter(|x| x.is_political_business_combo) {
        wtr.write_record([
            &r.name,
            &r.province,
            r.primary_category.as_cn(),
            &r.combo_brief,
            &r.reason,
            r.status.as_str(),
        ])?;
    }
    wtr.flush()?;
    Ok(())
}

fn draw_chart(out_dir: &Path, counts: &BTreeMap<String, usize>) -> Result<()> {
    let path = out_dir.join("chart.png");
    let path_str = path.to_string_lossy().to_string();

    let root = BitMapBackend::new(&path_str, (1200, 800)).into_drawing_area();
    root.fill(&WHITE)?;

    let categories: Vec<String> = Category::all_cn().iter().map(|s| s.to_string()).collect();
    let max_count = counts.values().copied().max().unwrap_or(0);
    let y_max = (max_count as i32 + 2).max(5);

    let mut chart = ChartBuilder::on(&root)
        .caption("代表身份分类统计", ("sans-serif", 36))
        .margin(30)
        .x_label_area_size(80)
        .y_label_area_size(60)
        .build_cartesian_2d(0..categories.len() as i32, 0..y_max)?;

    chart
        .configure_mesh()
        .disable_mesh()
        .x_labels(categories.len())
        .x_label_formatter(&|x| {
            let idx = *x as usize;
            if idx < categories.len() {
                categories[idx].clone()
            } else {
                "".to_string()
            }
        })
        .y_desc("人数")
        .x_desc("类别")
        .axis_desc_style(("sans-serif", 22))
        .label_style(("sans-serif", 18))
        .draw()?;

    for (idx, cat) in categories.iter().enumerate() {
        let count = *counts.get(cat).unwrap_or(&0) as i32;
        chart.draw_series(std::iter::once(Rectangle::new(
            [(idx as i32, 0), (idx as i32 + 1, count)],
            BLUE.filled(),
        )))?;

        chart.draw_series(std::iter::once(Text::new(
            count.to_string(),
            (idx as i32, count + 1),
            ("sans-serif", 20).into_font(),
        )))?;
    }

    root.present()?;
    Ok(())
}

fn build_npc_index_url(term: u32) -> String {
    format!("http://www.npc.gov.cn/npc/c191/dbmd/dbmd{}/", term)
}

fn clean_name(raw_text: &str) -> String {
    let re_paren = Regex::new(r#"(（[^）]*）|\([^)]*\))"#).unwrap();
    let text = re_paren.replace_all(raw_text, "");
    text.chars()
        .filter(|c| {
            !c.is_whitespace()
                && *c != '\u{00A0}'
                && *c != '\u{2002}'
                && *c != '\u{2003}'
                && *c != '\u{2009}'
                && *c != '\u{202F}'
                && *c != '\u{3000}'
        })
        .collect::<String>()
        .trim()
        .to_string()
}

async fn fetch_html(client: &Client, url: &str) -> Result<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("请求失败: {}", url))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("请求失败 {}: {}", status, url));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let bytes = resp.bytes().await.context("读取响应体失败")?;

    if content_type.contains("charset=utf-8") || content_type.contains("charset=\"utf-8\"") {
        return String::from_utf8(bytes.to_vec())
            .map_err(|e| anyhow!("按 UTF-8 解码失败: {}", e));
    }
    if content_type.contains("charset=gbk")
        || content_type.contains("charset=gb2312")
        || content_type.contains("charset=gb18030")
    {
        let (text, _, _) = encoding_rs::GBK.decode(&bytes);
        return Ok(text.into_owned());
    }

    let head_probe_len = bytes.len().min(4096);
    let probe = String::from_utf8_lossy(&bytes[..head_probe_len]).to_lowercase();

    if probe.contains("charset=utf-8") {
        return String::from_utf8(bytes.to_vec())
            .map_err(|e| anyhow!("按 UTF-8 解码失败: {}", e));
    }
    if probe.contains("charset=gbk")
        || probe.contains("charset=gb2312")
        || probe.contains("charset=gb18030")
    {
        let (text, _, _) = encoding_rs::GBK.decode(&bytes);
        return Ok(text.into_owned());
    }

    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
        return Ok(text);
    }

    let (text, _, _) = encoding_rs::GBK.decode(&bytes);
    Ok(text.into_owned())
}

fn parse_index_page(html: &str, base_url: &str) -> Result<Vec<(String, String)>> {
    let doc = Html::parse_document(html);
    let container_selector = Selector::parse(".md_all .md_zi a, .md_all .md_zi2 a")
        .map_err(|e| anyhow!("解析索引页选择器失败: {e}"))?;

    let mut result = Vec::new();
    for a in doc.select(&container_selector) {
        let province = a.text().collect::<String>().trim().to_string();
        let href = a.value().attr("href").unwrap_or("").trim();
        if province.is_empty() || href.is_empty() {
            continue;
        }
        let detail_url = if href.starts_with("http://") || href.starts_with("https://") {
            href.to_string()
        } else {
            format!("{}{}", base_url.trim_end_matches('/'), href)
        };
        result.push((province, detail_url));
    }

    if result.is_empty() {
        return Err(anyhow!("未从索引页解析出任何代表团链接"));
    }

    Ok(result)
}

fn parse_detail_page(html: &str) -> Result<(String, Vec<String>)> {
    let doc = Html::parse_document(html);
    let province_selector =
        Selector::parse(".nav_bt2").map_err(|e| anyhow!("解析详情页选择器失败: {e}"))?;
    let name_selector =
        Selector::parse(".md_all .md_zi").map_err(|e| anyhow!("解析详情页姓名选择器失败: {e}"))?;
    let re_count = Regex::new(r#"(\(\d+名\)|（\d+名）)"#).unwrap();

    let province_raw = doc
        .select(&province_selector)
        .next()
        .map(|n| n.text().collect::<String>())
        .unwrap_or_else(|| "未知".to_string());
    let province = re_count
        .replace_all(province_raw.trim(), "")
        .trim()
        .to_string();

    let mut names = Vec::new();
    for node in doc.select(&name_selector) {
        let raw = node.text().collect::<String>();
        let name = clean_name(&raw);
        if !name.is_empty() {
            names.push(name);
        }
    }

    if names.is_empty() {
        return Err(anyhow!("详情页未解析出任何代表姓名"));
    }

    Ok((province, names))
}

fn write_delegates_csv(out_dir: &Path, delegates: &[DelegateInput]) -> Result<PathBuf> {
    let path = delegates_csv_path(out_dir);
    let mut wtr = WriterBuilder::new()
        .from_path(&path)
        .with_context(|| format!("写入失败: {}", path.display()))?;

    wtr.write_record(["姓名", "省份"])?;
    for d in delegates {
        wtr.write_record([&d.name, &d.province])?;
    }
    wtr.flush()?;
    Ok(path)
}

async fn crawl_delegates(client: &Client, term: u32, out_dir: &Path) -> Result<Vec<DelegateInput>> {
    let index_url = build_npc_index_url(term);
    println!("正在抓取人大第 {} 届代表名单索引页: {}", term, index_url);

    let index_html = fetch_html(client, &index_url).await?;
    let province_links = parse_index_page(&index_html, "http://www.npc.gov.cn")?;
    println!("共发现 {} 个代表团页面", province_links.len());

    let mut delegates = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for (idx, (province_hint, url)) in province_links.iter().enumerate() {
        println!("[抓取 {}/{}] {} -> {}", idx + 1, province_links.len(), province_hint, url);

        match fetch_html(client, url).await {
            Ok(html) => match parse_detail_page(&html) {
                Ok((province, names)) => {
                    let province = if province == "未知" || province.is_empty() {
                        province_hint.clone()
                    } else {
                        province
                    };

                    println!("  -> 成功解析 {} 人", names.len());

                    for name in names {
                        delegates.push(DelegateInput {
                            name,
                            province: province.clone(),
                        });
                    }
                }
                Err(e) => {
                    eprintln!("  !! 解析失败：{} | {} | {}", province_hint, url, e);
                    failures.push(format!("解析失败：{} | {} | {}", province_hint, url, e));
                }
            },
            Err(e) => {
                eprintln!("  !! 抓取失败：{} | {} | {}", province_hint, url, e);
                failures.push(format!("抓取失败：{} | {} | {}", province_hint, url, e));
            }
        }
    }

    delegates.sort_by(|a, b| a.province.cmp(&b.province).then(a.name.cmp(&b.name)));
    delegates.dedup_by(|a, b| a.name == b.name && a.province == b.province);

    let path = write_delegates_csv(out_dir, &delegates)?;
    println!("已生成代表名单 CSV: {}", path.display());

    if !failures.is_empty() {
        eprintln!();
        eprintln!("以下页面抓取或解析失败（共 {} 个）：", failures.len());
        for item in &failures {
            eprintln!("  - {}", item);
        }
    }

    if delegates.is_empty() {
        return Err(anyhow!("所有详情页都抓取失败，delegates.csv 为空"));
    }

    Ok(delegates)
}

#[tokio::main]
async fn main() -> Result<()> {
    let dir = exe_dir()?;
    println!("程序目录: {}", dir.display());

    let out_dir = output_dir(&dir)?;
    println!("输出目录: {}", out_dir.display());

    let cfg = read_config(&dir)?;
    println!(
        "已读取配置。provider={}, model={}, max_concurrency={}, npc_term={}",
        cfg.provider, cfg.model, cfg.max_concurrency, cfg.npc_term
    );

    let safe_concurrency = cfg.max_concurrency.max(1);
    if safe_concurrency > 10 {
        eprintln!(
            "警告：当前 max_concurrency={} 偏高，建议降到 3~8，避免触发接口风控或限流。",
            safe_concurrency
        );
    }

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .context("创建 HTTP 客户端失败")?;

    let delegates = crawl_delegates(&client, cfg.npc_term, &out_dir).await?;
    if delegates.is_empty() {
        return Err(anyhow!("抓取得到的 delegates.csv 为空"));
    }

    let csv_path = delegates_csv_path(&out_dir);
    println!("使用名单文件: {}", csv_path.display());

    let delegates = read_delegates(&csv_path)?;
    if delegates.is_empty() {
        return Err(anyhow!("CSV 中没有可处理的有效记录"));
    }

    let mut cache = load_cache(&out_dir)?;
    println!("已加载缓存 {} 条", cache.len());

    let total = delegates.len();
    println!("共读取 {} 条有效记录，开始处理...", total);

    let semaphore = Arc::new(Semaphore::new(safe_concurrency));
    let fatal_auth_error = Arc::new(AtomicBool::new(false));

    let mut results: Vec<Option<LlmResult>> = vec![None; total];
    let mut tasks = Vec::new();

    for (idx, d) in delegates.iter().enumerate() {
        let key = cache_key(&d.name, &d.province);

        if let Some(hit) = cache.get(&key).cloned() {
            println!(
                "[{}/{}] 缓存命中：{} - {} -> {} ({})",
                idx + 1,
                total,
                d.name,
                d.province,
                hit.primary_category.as_cn(),
                hit.status.as_str()
            );
            results[idx] = Some(hit);
            continue;
        }

        let permit = semaphore.clone();
        let client = client.clone();
        let cfg_ref = cfg.clone();
        let name = d.name.clone();
        let province = d.province.clone();
        let fatal_auth_error_ref = fatal_auth_error.clone();

        tasks.push(tokio::spawn(async move {
            if fatal_auth_error_ref.load(Ordering::Relaxed) {
                return Ok::<(usize, Option<LlmResult>), anyhow::Error>((idx, None));
            }

            let _permit = permit.acquire_owned().await.map_err(|e| anyhow!(e))?;

            if fatal_auth_error_ref.load(Ordering::Relaxed) {
                return Ok::<(usize, Option<LlmResult>), anyhow::Error>((idx, None));
            }

            println!("[{}/{}] 正在处理：{} - {}", idx + 1, total, name, province);

            match call_llm_with_retry(&client, &cfg_ref, &name, &province).await {
                Ok(r) => {
                    println!(
                        "  -> 分类: {} | 政商合一: {} | {} | status={}",
                        r.primary_category.as_cn(),
                        r.is_political_business_combo,
                        r.name,
                        r.status.as_str()
                    );
                    Ok((idx, Some(r)))
                }
                Err(ApiError::Unauthorized(msg)) => {
                    fatal_auth_error_ref.store(true, Ordering::Relaxed);
                    Err(anyhow!("检测到 401/认证失败，已停止后续分类任务：{}", msg))
                }
                Err(ApiError::Retryable(msg)) | Err(ApiError::NonRetryable(msg)) => {
                    eprintln!("  !! 处理失败：{}，本条跳过，不写入缓存。错误：{}", name, msg);
                    Ok((idx, None))
                }
            }
        }));
    }

    let task_results = stream::iter(tasks)
        .buffer_unordered(safe_concurrency)
        .collect::<Vec<_>>()
        .await;

    let mut auth_failed = false;

    for task_result in task_results {
        match task_result {
            Ok(Ok((idx, maybe_result))) => {
                if let Some(result) = maybe_result {
                    if should_cache(&result) {
                        let key = cache_key(&result.name, &result.province);
                        cache.insert(key, result.clone());
                    }
                    results[idx] = Some(result);
                }
            }
            Ok(Err(e)) => {
                let msg = e.to_string();
                eprintln!("任务执行失败: {}", msg);
                if msg.contains("401") || msg.contains("认证失败") {
                    auth_failed = true;
                }
            }
            Err(e) => {
                eprintln!("任务 Join 失败: {}", e);
            }
        }
    }

    let final_results: Vec<LlmResult> = results.into_iter().flatten().collect();

    save_cache(&out_dir, &cache)?;
    write_results_csv(&out_dir, &final_results)?;
    write_results_json(&out_dir, &final_results)?;
    write_combo_csv(&out_dir, &final_results)?;
    let counts = write_summary_csv(&out_dir, &final_results)?;
    draw_chart(&out_dir, &counts)?;

    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    println!();
    println!("处理完成：{}", now);
    println!("成功写出 {} 条结果。", final_results.len());
    if auth_failed {
        eprintln!("注意：本次运行中检测到 401/认证失败，后续部分记录可能未被处理。");
        eprintln!("建议：");
        eprintln!("1. 将 config.json 中的 max_concurrency 降到 3~8");
        eprintln!("2. 检查 API key 是否正确、是否过期、是否触发平台风控");
        eprintln!("3. 重新运行程序，失败条目不会因缓存而被污染");
    }

    println!("输出文件：");
    println!("  - {}", out_dir.join("delegates.csv").display());
    println!("  - {}", out_dir.join("results.csv").display());
    println!("  - {}", out_dir.join("results.json").display());
    println!("  - {}", out_dir.join("summary.csv").display());
    println!("  - {}", out_dir.join("political_business_combo.csv").display());
    println!("  - {}", out_dir.join("chart.png").display());
    println!("  - {}", out_dir.join("cache.json").display());

    Ok(())
}