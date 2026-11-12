| Input | ops | make_patch: jsonpatch | jsonpatch-rs | speedup | apply_patch: jsonpatch | jsonpatch-rs | speedup |
|---|---:|---:|---:|---:|---:|---:|---:|
| config, 1 manifest (~60 KB) | 6 | 2.5 ms | 0.3 ms | **9×** | 0.7 ms | 0.2 ms | 3.1× |
| API snapshot, 1,000 records (0.2 MB), 5% changed | 61 | 44.8 ms | 4.4 ms | **10×** | 8.1 ms | 2.0 ms | 4.0× |
| API snapshot, 10,000 records (2.2 MB), 5% changed | 633 | 486.5 ms | 73.6 ms | **7×** | 85.4 ms | 22.5 ms | 3.8× |
| API snapshot, 50,000 records (10.9 MB), 5% changed | 3205 | 2,607.4 ms | 453.5 ms | **6×** | 465.4 ms | 126.4 ms | 3.7× |

Python 3.14.4, jsonpatch 1.47, x86_64, best of N runs, outputs compared for equality.
