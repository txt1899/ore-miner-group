# MinerGroup
一个开源的ore矿池项目

基本功能已经实现，但未做大量测试，用于正式环境自行测试。

后期优化希望大家共同努力，一起完善。欢迎提交PR。

### 正确的目录结构
```cmd
C:\USERS\USER_NAME\DESKTOP\ORE-MINER-GROUP
│  config.json
│  mine-client.exe
│  mine-server.exe
```

### 服务端配置文件
- `config.json`
  - `fee_payer`： gas支付钱包，省略使用矿工钱包
  - `dynamic_fee_url`：支持Helius、Alchemy、Quiknode、Triton

```json
{
  "rpc": "https://rpc.com/",
  "keypair_path": "I:/id.json",
  "fee_payer": "I:/id.json",
  "buffer_time": 5,
  "dynamic_fee_url": "https://rpc.com/"
}

```

### 服务端启动
```cmd

# --priority_fee：固定优先费
# --dynamic_fee： 是否动态优先费

# 固定优先费

.\mine-server.exe --priority_fee 50000

# 动态优先费，动态异常使用固定值

.\mine-server.exe --priority_fee 50000 --dynamic_fee
```


### 客户端启动
```cmd
# --url:    服务器地址
# --cores:  核心数（不提供使用全部核心）
# --wallet: 预留（随便填）

.\mine-client.exe --url "ws://127.0.0.1:8080" --cores 16 --wallet "" 

```