# Benchmark of Proposal Type Script

Unlike a normal script on CKB, the proposal type script needs to perform calculations over a large number of blocks, which could become a bottleneck. Hence we need to design a benchmark and measure it.

## Probe
We add USDT (User Statically-Defined Tracing) to the proposal type script for this task. We define the following probes:

```rust
#[usdt::provider]
pub mod proposal_probe {
    fn verify_entry() {}
    fn verify_exit() {}
    fn block_provider_entry() {}
    fn block_provider_exit() {}
}
```

They measure the `verify` function and block loading. The former gives an overview, while block loading reveals where the bottleneck lies.

## Steps to Bench
1. Build ckb with the `probe` feature enabled (it is enabled by default).
2. Run `e2e/start.sh` to start ckb.
3. Run `e2e/benchmark.sh` to start the benchmark.
4. Run `e2e/run-devnet.sh` to start the test case. Use `export DURATION=N` to specify the duration to measure.
5. Press `Ctrl+C` on `benchmark.sh` when testing is done.

## Results
On a MacBook Air M4, the results are as follows(DURATION=50):
```

╔════════════════════════════════════════════════════╗
║                verify()  Summary                   ║
╚════════════════════════════════════════════════════╝
  Total count:  2
  Total time:   385417 ns
  Average:      192708 ns
  Min:          2917 ns
  Max:          382500 ns

  Latency distribution (ns):


           value  ------------- Distribution ------------- count    
            1024 |                                         0        
            2048 |@@@@@@@@@@@@@@@@@@@@                     1        
            4096 |                                         0        
            8192 |                                         0        
           16384 |                                         0        
           32768 |                                         0        
           65536 |                                         0        
          131072 |                                         0        
          262144 |@@@@@@@@@@@@@@@@@@@@                     1        
          524288 |                                         0        


╔════════════════════════════════════════════════════╗
║          BlockProvider calls  Summary              ║
╚════════════════════════════════════════════════════╝
  Total count:  53
  Total time:   302998 ns
  Average:      5716 ns
  Min:          375 ns
  Max:          35792 ns

  Latency distribution (ns):


           value  ------------- Distribution ------------- count    
             128 |                                         0        
             256 |@@                                       2        
             512 |                                         0        
            1024 |                                         0        
            2048 |                                         0        
            4096 |@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@    49       
            8192 |@                                        1        
           16384 |                                         0        
           32768 |@                                        1        
           65536 |                                         0  

```

The result is 382,500 ns for 50 blocks. Normalized to 1 day (assuming 10 seconds per block), it is 66 ms. For 7 days: 463 ms.
The bottleneck is block loading, which consumed 79% of the time.
