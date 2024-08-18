# MinerGroup

一个开源的ore矿池项目

基本功能已经实现，但未做大量测试，用于正式环境自行测试。

后期优化希望大家共同努力，一起完善。欢迎提交PR。

[Discord](https://discord.gg/jsgwCwbU)  方便推送更新的通知

### 使用

```cmd
git clone https://github.com/txt1899/ore-miner-group.git

cd ore-miner-group

cargo build --release
```

### 目录结构

```cmd
C:\USERS\USER_NAME\DESKTOP\ORE-MINER-GROUP
│  config.json
│  benchmark.exe
│  mine-client.exe
│  mine-server.exe
```

### 服务端配置文件

- `config.json`
- `fee_payer`： gas支付钱包，省略使用矿工钱包
- `dynamic_fee_url`：支持Helius、Alchemy、Quiknode、Triton
- `jito_url`:
  jito节点地址（可省略默认主网）[Jito URL](https://jito-labs.gitbook.io/mev/searcher-resources/json-rpc-api-reference/url)。
- `port`： 服务端口

```json
{
  "rpc": "https://rpc.com/",
  "keypair_path": "I:/id.json",
  "fee_payer": "I:/id.json",
  "buffer_time": 5,
  "dynamic_fee_url": "https://rpc.com/",
  "jito_url": "https://mainnet.block-engine.jito.wtf/api/v1/transactions",
  "port": 8080
}

```

### 服务端启动

```cmd

# --priority-fee： 固定优先费
# --dynamic-fee：  是否动态优先费
# --jito：         是否使用jito

.\mine-server.exe --priority-fee 50000 --dynamic-fee --jito
```

### 客户端启动

```cmd
# --url:        服务器地址
# --reconnect:  重连次数（默认10次，每次间隔10秒）
# --cores:      核心数（不提供使用全部核心）
# --wallet:     预留（随便填）

.\mine-client.exe --url "ws://127.0.0.1:8080" --reconnect 10 --cores 16 --wallet "any" 

```

### 脚本定义gas && jito tip 

使用lua脚本通过自定义算法实现动态gas和tip。

这是一个正在验证的功能，需要你有一定的编程能力，请谨慎使用。

需要将`script.lua`文件放到`mine-server`同目录下，启动`server`时有确认提醒。

[Lua Online](https://onecompiler.com/lua/42pjgqps3)

```lua

-- dynamic_gas
-- 动态计算优先gas
-- difficulty: 当前难度
-- gas: 当前gas
-- return: u64
function dynamic_gas(difficulty, gas_lamports)
    return (difficulty - 10) * 1000 + gas_lamports;
end

-- dynamic_tip
-- 动态计算jit小费
-- difficulty: 当前难度
-- tip_lamports: 当前jito小费
-- return: u64
function dynamic_tip(difficulty, tip_lamports)
    return math.ceil((1 + (difficulty - 10) / 100) * tip_lamports);
end
```