# OreMinerGroup  
[![Discord](https://img.shields.io/badge/Discord-7289DA?logo=discord&logoColor=white)](https://discord.gg/Bz9qzXhAgJ)

### V2 将实现App, Server, Client的分离模式，提高账号安全性和灵活性。

本项目采用半开源的方式：

- App：
  - 开源
  - 负责管理矿工账号和提交交易，所有私钥不会上传到服务端。


- Client： 
  - 开源
  - 负责计算Hash，
  - 采用订阅方式保证任务的连续性，提高CPU利用率。


- Server:
  - 闭源
  - 接收App矿工的挑战任务
  - 派发任务到所有在线Client
  - 收集最后解决方案提供给矿工


- 说明 
  - #### _项目收费功能尚未实施_
  - 会以类似jito消费的方式收取，费用与难度成正比关系且有上限。

## Usage:


### APP

```cmd
# App目录结构

~\ORE-MINER-GROUP
│  config.json
│  app
│  account
   │ id_1.json
   │ id_2.json
   │ id_3.json
```

- `config.json`


- `user`：应用端用户名（如果要开启多个，不要重复）
- `server_host`：服务端端口地址
- `rpc`: RPC
- `dynamic_fee_url`:支持动态计算的RPC, 支持Helius、Alchemy、Quiknode、Triton
- `fee_payer`：as支付钱包，省略使用矿工钱包，
```json
{
  "user": "test app",
  "server_host": "127.0.0.1:8080",
  "rpc": "https://prc.com/",
  "jito_url": "https://mainnet.block-engine.jito.wtf/api/v1/transactions",
  "dynamic_fee_url": "https://prc.com/",
  "fee_payer": "~/.config/solana/id.json"
}
```


```shell
# account 存放矿工keypair的文件夹
./account/*.json
```

```shell
# --priority-fee： 固定优先费
# --dynamic-fee：  是否动态优先费
# --jito：         是否使用jito
./app --priority-fee 1000 --dynamic-fee --jito
```


### Client

```shell
# --host:     服务端端口地址
# --cores:    核心数（不提供使用全部核心）
# --wallet:   预留（随便填）

./client --host "127.0.0.1:8080" --cores 15 --reconnect 10 --wallet "any"
```

### Server

```shell
# --port: 服务端口
./server --port 8080
```



## 项目进度

### App
![95%](https://geps.dev/progress/95)
### Server
![95%](https://geps.dev/progress/95)
### Client
![95%](https://geps.dev/progress/85)


