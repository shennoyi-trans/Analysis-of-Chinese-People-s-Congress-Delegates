use anyhow::{anyhow, Context, Result};
use chrono::Local;
use csv::{ReaderBuilder, WriterBuilder};
use futures::stream::{self, StreamExt};
use plotters::prelude::*;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Debug, Deserialize)]
struct Config {
    provider: String,
    api_key: String,
    model: String,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default = "default_max_concurrency")]
    max_concurrency: usize,
}

fn default_max_concurrency() -> usize {
    5
}

#[derive(Debug, Deserialize)]
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
struct LlmResult {
    name: String,
    province: String,
    primary_category: Category,
    is_political_business_combo: bool,
    combo_brief: String,
    reason: String,
    confidence: Option<f64>,
    sources: Vec<String>,
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
}

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

fn cache_key(name: &str, province: &str) -> String {
    format!("{}||{}", name.trim(), province.trim())
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

fn is_output_or_generated_csv(path: &Path) -> bool {
    let filename = path
        .file_name()
        .map(|x| x.to_string_lossy().to_string())
        .unwrap_or_default()
        .to_ascii_lowercase();

    filename.starts_with("results")
        || filename.starts_with("summary")
        || filename.starts_with("political_business_combo")
        || filename == "cache.csv"
}

fn find_csv_file(dir: &Path) -> Result<PathBuf> {
    let mut csv_files = vec![];
    for entry in fs::read_dir(dir).with_context(|| format!("读取目录失败: {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext.to_string_lossy().to_ascii_lowercase() == "csv"
                    && !is_output_or_generated_csv(&path)
                {
                    csv_files.push(path);
                }
            }
        }
    }

    match csv_files.len() {
        0 => Err(anyhow!("同目录下未找到可用的 CSV 名单文件")),
        1 => Ok(csv_files.remove(0)),
        _ => {
            csv_files.sort();
            Ok(csv_files.remove(0))
        }
    }
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

        // 跳过空行 / 无姓名行，避免多余调用
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
r#"你是一个严谨的信息分类助手。

任务：
请对“人大代表”进行身份识别。你必须优先联网搜索公开资料（官方简历、政府网站、新闻报道、权威媒体等），再给出结论。

输入信息：
- 人名：{name}
- 省份/代表团：{province}

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

{{
  "name": "{name}",
  "province": "{province}",
  "primary_category": "党政干部|企业家|工农基层代表|解放军和武警系统代表|其他各行业各领域代表|未知",
  "is_political_business_combo": true,
  "combo_brief": "若非政商合一可写空字符串",
  "reason": "简要说明",
  "confidence": 0.0,
  "sources": ["来源1", "来源2"]
}}

注意：
- confidence 取值 0 到 1
- 如果无法确认，请降低 confidence
- 必须保证 JSON 合法
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

async fn call_llm(client: &Client, cfg: &Config, name: &str, province: &str) -> Result<LlmResult> {
    let prompt = build_prompt(name, province);

    let base_url = cfg
        .base_url
        .clone()
        .unwrap_or_else(|| "https://api.deepseek.com".to_string());

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let body = json!({
        "model": cfg.model,
        "messages": [
            {"role": "system", "content": "你必须严格输出 JSON。"},
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
        .with_context(|| format!("LLM 请求失败: {}", name))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("LLM 接口返回错误 {}: {}", status, text));
    }

    let v: Value = resp.json().await.context("解析 LLM 响应 JSON 失败")?;

    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("未找到模型输出内容"))?
        .trim()
        .to_string();

    parse_llm_output(&content, name, province)
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

    wtr.write_record(["name", "province", "primary_category", "combo_brief", "reason"])?;
    for r in results.iter().filter(|x| x.is_political_business_combo) {
        wtr.write_record([
            &r.name,
            &r.province,
            r.primary_category.as_cn(),
            &r.combo_brief,
            &r.reason,
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

#[tokio::main]
async fn main() -> Result<()> {
    let dir = exe_dir()?;
    println!("程序目录: {}", dir.display());

    let out_dir = output_dir(&dir)?;
    println!("输出目录: {}", out_dir.display());

    let cfg = read_config(&dir)?;
    println!(
        "已读取配置。provider={}, model={}, max_concurrency={}",
        cfg.provider, cfg.model, cfg.max_concurrency
    );

    let csv_path = find_csv_file(&dir)?;
    println!("使用名单文件: {}", csv_path.display());

    let delegates = read_delegates(&csv_path)?;
    if delegates.is_empty() {
        return Err(anyhow!("CSV 中没有可处理的有效记录"));
    }

    let mut cache = load_cache(&out_dir)?;
    println!("已加载缓存 {} 条", cache.len());

    let total = delegates.len();
    println!("共读取 {} 条有效记录，开始处理...", total);

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .context("创建 HTTP 客户端失败")?;

    let semaphore = Arc::new(Semaphore::new(cfg.max_concurrency));

    let mut results: Vec<Option<LlmResult>> = vec![None; total];
    let mut tasks = Vec::new();

    for (idx, d) in delegates.iter().enumerate() {
        let key = cache_key(&d.name, &d.province);

        if let Some(hit) = cache.get(&key).cloned() {
            println!(
                "[{}/{}] 缓存命中：{} - {} -> {}",
                idx + 1,
                total,
                d.name,
                d.province,
                hit.primary_category.as_cn()
            );
            results[idx] = Some(hit);
            continue;
        }

        let permit = semaphore.clone();
        let client = client.clone();
        let cfg_ref = Config {
            provider: cfg.provider.clone(),
            api_key: cfg.api_key.clone(),
            model: cfg.model.clone(),
            base_url: cfg.base_url.clone(),
            max_concurrency: cfg.max_concurrency,
        };
        let name = d.name.clone();
        let province = d.province.clone();

        tasks.push(tokio::spawn(async move {
            let _permit = permit.acquire_owned().await.map_err(|e| anyhow!(e))?;
            println!("[{}/{}] 正在处理：{} - {}", idx + 1, total, name, province);

            let result = match call_llm(&client, &cfg_ref, &name, &province).await {
                Ok(r) => {
                    println!(
                        "  -> 分类: {} | 政商合一: {} | {}",
                        r.primary_category.as_cn(),
                        r.is_political_business_combo,
                        r.name
                    );
                    r
                }
                Err(e) => {
                    eprintln!("  !! 处理失败：{}，已记为未知。错误：{}", name, e);
                    LlmResult {
                        name: name.clone(),
                        province: province.clone(),
                        primary_category: Category::Unknown,
                        is_political_business_combo: false,
                        combo_brief: "".to_string(),
                        reason: format!("处理失败：{}", e),
                        confidence: Some(0.0),
                        sources: vec![],
                    }
                }
            };

            Ok::<(usize, LlmResult), anyhow::Error>((idx, result))
        }));
    }

    let task_results = stream::iter(tasks)
        .buffer_unordered(cfg.max_concurrency)
        .collect::<Vec<_>>()
        .await;

    for task_result in task_results {
        match task_result {
            Ok(Ok((idx, result))) => {
                let key = cache_key(&result.name, &result.province);
                cache.insert(key, result.clone());
                results[idx] = Some(result);
            }
            Ok(Err(e)) => {
                eprintln!("任务执行失败: {}", e);
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
    println!("输出文件：");
    println!("  - {}", out_dir.join("results.csv").display());
    println!("  - {}", out_dir.join("results.json").display());
    println!("  - {}", out_dir.join("summary.csv").display());
    println!(
        "  - {}",
        out_dir.join("political_business_combo.csv").display()
    );
    println!("  - {}", out_dir.join("chart.png").display());
    println!("  - {}", out_dir.join("cache.json").display());

    Ok(())
}