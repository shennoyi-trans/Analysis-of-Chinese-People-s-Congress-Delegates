# 🧠 Delegate Classifier (人大代表身份分类工具)

一个基于 LLM 的自动化工具，用于：

- 批量读取代表名单（CSV）
    
- 自动查询代表公开信息（通过 LLM）
    
- 分类其身份结构
    
- 检测“政商合一”情况
    
- 生成统计结果与可视化图表
    

---

## ✨ 功能特点

### 🔍 自动身份分类

对每位代表进行分类：

- 党政干部
    
- 企业家
    
- 工农基层代表
    
- 解放军和武警系统代表
    
- 其他各行业各领域代表
    
- 未知
    

---

### ⚠️ 政商合一识别（重点功能）

- 自动识别同时具备 **政界 + 商界身份** 的代表
    
- 单独输出名单
    
- 提供简要说明
    

---

### ⚡ 并行调用 LLM

- 支持并发请求（可配置）
    
- 显著提升处理速度
    

---

### 💾 缓存机制

- 自动缓存已查询结果（`cache.json`）
    
- 避免重复请求，降低成本
    
- 支持增量运行
    

---

### 📊 可视化输出

自动生成：

- 分类统计表
    
- 柱状图（PNG）
    

---

## 📂 项目结构

```text
your_folder/
├─ delegate_classifier.exe   # 编译后的程序
├─ config.json              # LLM配置
├─ delegates.csv            # 输入名单
├─ output/                  # 自动生成
│   ├─ results.csv
│   ├─ results.json
│   ├─ summary.csv
│   ├─ political_business_combo.csv
│   ├─ chart.png
│   └─ cache.json
```

---

## 🚀 使用方法

### 1️⃣ 准备 CSV 名单

支持以下格式：

```csv
name,province
张三,北京
李四,上海
```

或中文表头：

```csv
姓名,省份
张三,北京
李四,上海
```

---

### 2️⃣ 配置 `config.json`

示例：

```json
{
  "provider": "deepseek",
  "api_key": "你的API_KEY",
  "model": "deepseek-chat",
  "base_url": "https://api.deepseek.com",
  "max_concurrency": 5
}
```

参数说明：

|字段|说明|
|---|---|
|provider|LLM提供商（仅记录用）|
|api_key|API Key|
|model|使用的模型|
|base_url|接口地址|
|max_concurrency|最大并发数|

---

### 3️⃣ 运行程序

#### 开发模式

```bash
cargo run --release
```

#### 或直接运行可执行文件

```bash
./delegate_classifier
```

---

### 4️⃣ 查看输出

所有结果会生成在：

```text
/output
```

---

## 📊 输出文件说明

### `results.csv`

完整结果：

|字段|说明|
|---|---|
|name|姓名|
|province|省份|
|primary_category|分类|
|is_political_business_combo|是否政商合一|
|combo_brief|简要说明|
|reason|判断依据|
|confidence|置信度|
|sources|信息来源|

---

### `summary.csv`

分类统计汇总

---

### `political_business_combo.csv`

仅包含“政商合一”代表

---

### `chart.png`

分类柱状图

---

### `cache.json`

缓存数据（自动生成）

---

## ⚙️ 关键设计说明

### 🔹 严格 JSON 输出约束

通过提示词强制 LLM 输出标准 JSON，便于程序解析。

---

### 🔹 空行过滤

自动跳过 CSV 中的空行，避免无效调用。

---

### 🔹 并发控制

使用 Semaphore 控制并发：

```text
总耗时 ≈ 数据量 / 并发数
```

---

### 🔹 缓存键设计

```text
name || province
```

确保同一代表不会重复请求。

---

## ⚠️ 注意事项

### ❗ 关于“联网搜索”

本程序会在提示词中要求 LLM：

> “优先联网搜索公开资料”

但：

- 是否真正联网 **取决于你使用的模型**
    
- 若模型不支持联网，结果可能基于训练数据推断
    

---

### ❗ 分类不保证绝对准确

该工具适用于：

- 数据探索
    
- 初步统计
    
- 结构分析
    

不适用于：

- 严格学术统计
    
- 官方数据发布
      

---

## 🧩 技术栈

- Rust (Tokio async)
    
- Reqwest (HTTP)
    
- Serde (JSON)
    
- CSV crate
    
- Plotters (图表)
    

---

## 📜 License

MIT

---

## 👀 示例效果

运行后你会得到：

- 📄 自动分类数据
    
- 📊 分类统计图
    
- ⚠️ 政商合一名单
    