# OreMinerGroup  
[![Discord](https://img.shields.io/badge/Discord-7289DA?logo=discord&logoColor=white)](https://discord.gg/Bz9qzXhAgJ)

### V2 将实现App, Server, Client的分离模式，提高账号安全性和灵活性。
1. [x] APP: （开源）用于加载挖矿私钥，提高私钥的安全性。并调用Server的API就行挖矿。
2. [x] SERVER:（闭源）用于提供挖矿的API，生成挖矿任务并派发到Client。
3. [x] CLIENT:（开源）以订阅的方式接收挖矿任务，提高设备利用率。


## 项目进度

### App
![90](https://geps.dev/progress/90)
### Server
![90%](https://geps.dev/progress/90)
### Client
![95%](https://geps.dev/progress/95)



# Usage:

### APP


config.json
```json
{
  "user": "test app",
  "server_host": "127.0.0.1:8080",
  "rpc": "https://amaleta-5y8tse-fast-mainnet.helius-rpc.com/",
  "jito_url": "https://mainnet.block-engine.jito.wtf/api/v1/transactions",
  "dynamic_fee_url": "https://mainnet.block-engine.jito.wtf/api/v1/transactions",
  "fee_payer": "~/.config/solana/id.json"
}
```

miner keypair id
```tree
./account/*.json
```

cmd:
```shell
./app --priority-fee 1000 --dynamic-fee --jito
```


### Client

```shell
./client --host "127.0.0.1:8080" --cores 15 --reconnect 20 --wallet "9emUg5NCYop62HnFXkPyhgVMEwfi3TXyYae5xoCgn4mw"
```