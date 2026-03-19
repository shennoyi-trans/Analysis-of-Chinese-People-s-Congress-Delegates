# Tool-LLM-based-Analysis-of-Chinese-People-s-Congress-Delegates
📌 项目简介

本项目是一个基于大语言模型（LLM）的自动化分析工具，用于对中国全国人大代表进行身份识别、分类，并生成结构化数据与可视化结果。

通过结合：
LLM推理能力
联网搜索信息
省份信息去重校验
结构化输出约束
实现对代表身份的半自动化、可扩展分析。

🎯 项目目标

传统统计人大代表结构（如“官员占比”“企业家占比”）存在问题：
数据不公开或不细分
手动整理成本极高
同名歧义严重
政商身份交叉难以识别

本项目尝试用 LLM + Prompt Engineering 解决：
✅ 自动检索信息
✅ 自动判定身份类别
✅ 自动识别“政商合一”
✅ 自动生成统计与可视化

🧩 分类体系

每位代表被归类为以下 6 类之一：
党政干部
企业家
工农基层代表
解放军和武警系统代表
其他各行业各领域代表
未知

此外：
👉 若同时具备政治 + 企业身份，会额外标记为：
“政商合一”

⚙️ 功能特性

🔍 自动调用 LLM 进行联网搜索与信息判断
🧠 Prompt 约束严格 JSON 输出（便于程序解析）
🧭 基于“省份”进行同名去重校验
⚠️ 自动降级为“未知”避免误判

📊 自动生成统计数据与图表：
柱状图
饼图

📁 输出多种格式：
CSV（可读）
JSONL（可编程处理）
🧾 自动导出“政商合一名单”

📂 输入格式
使用 CSV：
name,province
张三,北京市
李四,广东省
王五,浙江省

只需两列：
name：姓名
province：所属省份（用于去重）

🚀 使用方法

1️⃣ 安装依赖
pip install matplotlib openai

2️⃣ 准备配置文件
示例：
{
  "provider": "deepseek",
  "api_key": "YOUR_API_KEY",
  "model": "deepseek-chat"
}

3️⃣ 运行脚本
python classify_delegates_standalone.py \
  --input delegates.csv \
  --output-dir output \
  --config-file config.json
  
📊 输出结果
运行后将生成：
文件	说明
classified_results.csv	每位代表的分类结果
classified_results.jsonl	原始结构化数据
overlap_politics_business.csv	政商合一名单
summary_counts.csv	分类统计
category_bar.png	柱状图
category_pie.png	饼图

⚠️ 注意事项

❗ 模型是否真的“联网搜索”，取决于所使用的 LLM 服务能力
❗ 分类结果是“推理结果”，并非官方统计
❗ 存在误判可能，尤其是：
同名人物
信息较少的代表
职业跨界人员

🧪 适用场景
人大结构研究
政商关系分析
社会结构数据挖掘
LLM 信息抽取实验
Prompt Engineering 实践
