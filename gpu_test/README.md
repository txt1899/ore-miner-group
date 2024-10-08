# OreMinerGroup —— 支持Jito捆绑交易
[![Discord](https://img.shields.io/badge/Discord-7289DA?logo=discord&logoColor=white)](https://discord.gg/Bz9qzXhAgJ)

## Drillx GPU 基准测试

| GPU  | Hash/Sec |
|------|----------|
| 3060 | 2400+    |
| 4090 | 8000+    |

````rust
use std::time::Instant;
use drillx_gpu::BATCH_SIZE;
use drillx::Solution;

fn hashspace_size() -> usize {
    unsafe { BATCH_SIZE as usize * 3 * 28 }
}

fn main() {
    let challenge = [0; 32];
    let mut nonce = [0; 8];
    let mut sols = vec![0u8; hashspace_size()];
    let mut size = 0i32;
    unsafe {
        let d = drillx_gpu::drillx_alloc(0);
        let timer = Instant::now();
        let mut total = 0;
        for n in 0..10_u64 {
            let t = Instant::now();
            nonce = n.to_le_bytes();
            drillx_gpu::hash(
                d,
                challenge.as_ptr(),
                nonce.as_ptr(),
                sols.as_mut_ptr() as *mut u8,
                &mut size as *mut i32
            );

            total += size;
            println!(
                "Gpu returned {} sols in {} ms",
                size,
                t.elapsed().as_millis()
            );

            for i in 0..size as usize {
                let data = &sols[i * 28..(i + 1) * 28];

                let d = &data[0..16];      // digest
                let n = &data[16..24];     // nonce
                let _f  = &data[24..28];   // difficulty

                let solution = Solution::new(
                    d.try_into().unwrap(),
                    n.try_into().unwrap()
                );

                assert!(solution.is_valid(&challenge));
            }
        }

        let elapsed = timer.elapsed().as_secs_f32();
        let hashrate = total as f32 / elapsed;

        println!(
            "Get {} sols in {} ms, Speed: {:.2} H/s",
            total,
            timer.elapsed().as_millis(),
            hashrate
        );

        drillx_gpu::drillx_free(d);
    }
}
````


### WSL2.0 Ubuntu:
```shell
./gpu_benchmark

* ID: 0
* Name: NVIDIA GeForce RTX 3060
* Major: 8, Minor: 6
* Used Memory: 10745 MB
* Total Memory: 12287 MB

Kernel time = 1758.69 ms.
Gpu returned 4095 sols in 1760 ms
Kernel time = 1732.76 ms.
Gpu returned 4094 sols in 1733 ms
Kernel time = 1730.28 ms.
Gpu returned 4094 sols in 1730 ms
Get 12283 sols in 5811 ms, Speed: 2113.75 H/s
~drillx
```

### Windows:
```shell
./gpu_benchmark.exe

* ID: 0
* Name: NVIDIA GeForce RTX 3060
* Major: 8, Minor: 6
* Used Memory: 10694 MB
* Total Memory: 12287 MB

Kernel time = 1733.96 ms.
Gpu returned 4095 sols in 1734 ms
Kernel time = 1710.58 ms.
Gpu returned 4094 sols in 1710 ms
Kernel time = 1724.13 ms.
Gpu returned 4094 sols in 1724 ms
Get 12283 sols in 5815 ms, Speed: 2112.30 H/s
~drillx
```