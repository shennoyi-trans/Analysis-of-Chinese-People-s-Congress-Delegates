# Analysis-of-Chinese-People-s-Congress-Delegates  
  
一个用于批量分析全国人大代表身份构成的工具。程序会自动抓取指定届次的人大代表名单，调用 LLM 对每位代表进行身份分类，并生成统计结果与可视化图表。  
  
## 主要功能  
  
- 自动抓取指定届次全国人大代表名单  
- 调用 LLM 对代表身份进行分类  
- 输出分类结果、汇总统计和图表  
- 支持缓存，避免重复请求已处理过的数据  
- 支持并发处理，提高整体速度  
  
## 分类范围  
  
程序将代表划分为以下几类：  
  
- 党政干部  
- 企业家  
- 工农基层代表  
- 解放军和武警系统代表  
- 其他各行业各领域代表  
- 未知  
  
同时还会额外标记是否存在“政商合一”情况。  
  
---  
  
## 使用方法  
  
### 1. 配置 API Key  
  
打开程序同目录下的 `config.json` 文件，在其中填入你自己的 API Key。  
  
```json  
{  
  "provider": "deepseek",  
  "api_key": "your_api_key_here",  
  "model": "deepseek-chat",  
  "max_concurrency": 5,  
  "npc_term": 14  
}
```

其中：
#### `provider`

LLM 提供商名称。当前示例使用的是：

"provider": "deepseek"

#### `api_key`

你的 API Key，需要手动填写，否则程序无法调用模型接口。

"api_key": "your_api_key_here"

#### `model`

使用的模型名称，例如：

"model": "deepseek-chat"

请注意模型调用费用，以deepseek-chat为例，单次运行本程序约花费4￥。

#### `max_concurrency`

最大并发数，用于控制同时发起的请求数量，较大的数值可以缩短程序运行所需的时间。

"max_concurrency": 10

建议不要设得过高，否则可能触发接口限流、风控或认证异常。

#### `npc_term`

人大届次，例如：

"npc_term": 14

表示抓取第 14 届全国人大代表名单。

    

### 2. 启动程序

将 `delegate_classifier.exe` 和 `config.json` 放在同一目录下，直接**双击 `delegate_classifier.exe`** 即可运行。

也可在命令提示符中输入`delegate_classifier.exe`启动。

程序会自动：

1. 抓取人大代表名单
    
2. 调用 LLM 进行分类
    
3. 在 `output` 目录下生成结果文件
    

---

## 输出文件

程序运行后会在 `output` 目录下生成结果文件，包括：

- `delegates.csv`：抓取到的代表名单
    
- `results.csv`：分类结果表
    
- `results.json`：分类结果 JSON
    
- `summary.csv`：分类汇总统计
    
- `political_business_combo.csv`：政商合一代表列表
    
- `chart.png`：统计图表
    
- `cache.json`：缓存文件
    

---

## 注意事项

- 请确保 `config.json` 中已正确填写自己的 API Key
    
- 请确保 `delegate_classifier.exe` 与 `config.json` 位于同一目录
    
- 若修改了配置文件，保存后重新双击 exe 即可生效
    
- 若因接口报错导致结果异常，可删除 `output/cache.json` 后重新运行
    

---

## 适用场景

适合用于：

- 分析人大代表身份结构
    
- 观察不同类别代表占比
    
- 快速生成初步统计与可视化结果
    
- 为后续人工复核提供基础数据支持