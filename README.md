# 开源矿池 - OreMinerGroup —— 支持Jito捆绑交易
[![Discord](https://img.shields.io/badge/Discord-7289DA?logo=discord&logoColor=white)](https://discord.gg/Bz9qzXhAgJ)


## App, Server, Client的分离模式，提高账号安全性和灵活性。

- App：
  - 负责管理矿工账号和提交交易，所有私钥不会上传到服务端

- Client：
  - 负责计算Hash
  - 采用订阅方式保证任务的连续性，提高CPU利用率

- Server:
  - 接收App矿工的挑战任务
  - 派发任务到所有在线Client
  - 收集最后解决方案提供给矿工


## Usage:


### APP

```cmd
# App目录结构

~/ORE-MINER-GROUP
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
- `fee_payer`: 指定固定钱包支付手续费
```json
{
  "user": "test app",
  "server_host": "127.0.0.1:8080",
  "rpc": "https://prc.com/",
  "fee_payer": "~/.config/solana/id.json"
}
```


```shell
# account 存放矿工keypair的文件夹
./account/*.json
```

```shell
# --min-tip：       jito最低小费（默认：1000）
# --max-tip：       jito最高小费（默认：0，自适应）
# --bundle-buffer： 收集打包交易的缓冲时间，越小兼容性越强，当同时越也容易受到挖矿惩罚。
./app --min-tip 1000 --max-tip 5000 --bundle-buffer 5
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
