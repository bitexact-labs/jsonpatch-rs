"""python bench/bench.py  -> markdown table. Original jsonpatch vs jsonpatch_rs, same inputs, identical output checked."""
import sys, time, json, random, copy, statistics, os
sys.path.insert(0, os.path.dirname(__file__))
import jsonpatch, jsonpatch_rs, gen

def best(f, reps):
    ts = []
    for _ in range(reps):
        t = time.perf_counter(); out = f(); ts.append(time.perf_counter() - t)
    return min(ts), out

def config_pair(seed):
    """k8s-style manifest: nested dicts, small lists, a few edits"""
    r = random.Random(seed)
    def c(i): return {"name": f"c{i}", "image": f"repo/app:{r.randint(1,9)}", "env": [{"name": f"E{j}", "value": str(r.random())} for j in range(8)],
                      "resources": {"limits": {"cpu": "500m", "memory": "512Mi"}}, "ports": [{"containerPort": 8000 + j} for j in range(3)]}
    src = {"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "web", "labels": {f"l{i}": str(i) for i in range(20)}},
           "spec": {"replicas": 3, "template": {"spec": {"containers": [c(i) for i in range(30)]}}}}
    dst = copy.deepcopy(src)
    dst["spec"]["replicas"] = 5
    cs = dst["spec"]["template"]["spec"]["containers"]
    cs[3]["image"] = "repo/app:10"; cs[7]["env"].insert(2, {"name": "NEW", "value": "1"}); del cs[12]
    cs.insert(20, c(99)); dst["metadata"]["labels"]["new"] = "x"
    return src, dst

rows = []
cases = [("config, 1 manifest (~60 KB)", config_pair(1), 50)]
for n in (1000, 10000, 50000):
    src, dst = gen.pair(n)
    cases.append((f"API snapshot, {n:,} records ({len(json.dumps(src))/1e6:.1f} MB), 5% changed", (src, dst), 5 if n < 50000 else 2))
for name, (src, dst), reps in cases:
    t0, p0 = best(lambda: jsonpatch.make_patch(src, dst), reps)
    t1, p1 = best(lambda: jsonpatch_rs.make_patch(src, dst), reps)
    assert list(p0) == list(p1) and json.dumps(list(p0)) == json.dumps(list(p1)), name
    a0, r0 = best(lambda: jsonpatch.apply_patch(src, p0), reps)
    a1, r1 = best(lambda: jsonpatch_rs.apply_patch(src, p0), reps)
    assert r0 == dst and r1 == dst
    rows.append((name, len(p0.patch), t0, t1, a0, a1))
print("| Input | ops | make_patch: jsonpatch | jsonpatch-rs | speedup | apply_patch: jsonpatch | jsonpatch-rs | speedup |")
print("|---|---:|---:|---:|---:|---:|---:|---:|")
ms = lambda t: f"{t*1000:,.1f} ms"
for name, ops, t0, t1, a0, a1 in rows:
    print(f"| {name} | {ops} | {ms(t0)} | {ms(t1)} | **{t0/t1:.0f}×** | {ms(a0)} | {ms(a1)} | {a0/a1:.1f}× |")
import platform; print(f"\nPython {platform.python_version()}, jsonpatch {jsonpatch.__version__}, {platform.processor() or platform.machine()}, best of N runs, outputs compared for equality.")
