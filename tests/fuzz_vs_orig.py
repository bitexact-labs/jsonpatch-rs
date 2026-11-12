"""Fuzz: jsonpatch_rs must produce exactly the patch python-json-patch produces.
usage: fuzz_vs_orig.py SEED N"""
import sys, random, json, copy, math
import jsonpatch, jsonpatch_rs
from jsonpatch import JsonPatch, JsonPatchWithAppend

seed, N = int(sys.argv[1]), int(sys.argv[2])
r = random.Random(seed)
KEYS = ["a", "b", "c", "id", "x/y", "t~1", "~", "", "ключ", "é", "0", "1", "-"]
SCAL = [0, 1, 2, -1, True, False, None, 1.0, 0.0, -0.0, 2.5, "", "a", "ab", "abc", "1", "true", "日本", 10**15, 1e16]

def scalar():
    if r.random() < 0.15:
        return r.choice([r.randint(-5, 5), round(r.uniform(-3, 3), r.randint(0, 3)), "".join(r.choice("ab~/é") for _ in range(r.randint(0, 4)))])
    return r.choice(SCAL)

def value(d=0, big=False):
    k = r.random()
    if d > 4 or k < 0.45:
        return scalar()
    if k < 0.75:
        n = r.choice([0, 1, 2, 3, 5, 8]) if not big else r.randint(0, 700)
        pool = [value(d + 1) for _ in range(r.randint(1, 4))]
        return [copy.deepcopy(r.choice(pool)) if r.random() < 0.5 else value(d + 1) for _ in range(n)]
    return {r.choice(KEYS): value(d + 1) for _ in range(r.randint(0, 5))}

def mutate(v, d=0):
    v = copy.deepcopy(v)
    if r.random() < 0.08 or d > 5:
        return value(d)
    if isinstance(v, list):
        for _ in range(r.randint(0, 4)):
            op = r.random()
            if op < 0.2 and v: del v[r.randrange(len(v))]
            elif op < 0.4: v.insert(r.randint(0, len(v)), value(d + 1) if r.random() < 0.6 or not v else copy.deepcopy(r.choice(v)))
            elif op < 0.6 and v: i = r.randrange(len(v)); v.insert(r.randint(0, len(v) - 1), v.pop(i))
            elif op < 0.85 and v: i = r.randrange(len(v)); v[i] = mutate(v[i], d + 1)
            elif v: v.reverse() if r.random() < 0.3 else r.shuffle(v)
        return v
    if isinstance(v, dict):
        for _ in range(r.randint(0, 3)):
            op = r.random()
            if op < 0.25 and v: del v[r.choice(list(v))]
            elif op < 0.5: v[r.choice(KEYS)] = value(d + 1)
            elif op < 0.85 and v: k = r.choice(list(v)); v[k] = mutate(v[k], d + 1)
            elif v and r.random() < 0.5:  # move a member under another key
                k = r.choice(list(v)); v[r.choice(KEYS)] = v.pop(k)
        return v
    if isinstance(v, str) and r.random() < 0.4:
        return v + r.choice(["x", "é", "/", "~"])
    return scalar() if r.random() < 0.7 else v

def typed(x):
    """json with types kept apart (1 vs True vs 1.0 vs -0.0)"""
    if isinstance(x, bool): return ["b", x]
    if isinstance(x, int): return ["i", x]
    if isinstance(x, float): return ["f", repr(x)]
    if isinstance(x, list): return ["l", [typed(i) for i in x]]
    if isinstance(x, dict): return ["d", [[k, typed(v)] for k, v in x.items()]]
    return ["o", repr(type(x)), repr(x)]

fails = 0
checked = 0
for case in range(N):
    big = r.random() < 0.05
    src = value(0, big)
    if big and not isinstance(src, list):
        src = [value(1) for _ in range(r.randint(400, 900))]
    dst = mutate(src)
    if r.random() < 0.3:
        dst = mutate(dst)
    kw = {}
    if r.random() < 0.3: kw["generate_test_ops"] = True
    if r.random() < 0.2: kw["optimization"] = False
    cls = JsonPatchWithAppend if r.random() < 0.25 else JsonPatch
    s0, d0 = copy.deepcopy(src), copy.deepcopy(dst)
    try:
        want = list(jsonpatch_rs._orig_from_diff_append(cls, src, dst, **kw) if cls is JsonPatchWithAppend else jsonpatch_rs._orig_from_diff(cls, src, dst, **kw))
        werr = None
    except Exception as e:
        want, werr = None, repr(e)
    try:
        got_p = jsonpatch_rs._from_diff(cls, src, dst, **kw)
        got, gerr = list(got_p), None
        assert type(got_p) is cls
    except Exception as e:
        got, gerr = None, repr(e)
    checked += 1
    ok = (werr == gerr) and (want is None or typed(want) == typed(got))
    if ok and want is not None:
        # values must be copies, not the input objects
        for op in got:
            v = op.get("value")
            if isinstance(v, (list, dict)) and any(v is x for x in (src, dst)):
                ok = False
        if typed(src) != typed(s0) or typed(dst) != typed(d0):
            ok = False
    if not ok:
        fails += 1
        if fails <= 3:
            print("MISMATCH case", case, kw, cls.__name__)
            print(" src", json.dumps(src)[:400]); print(" dst", json.dumps(dst)[:400])
            print(" want", werr or json.dumps(want)[:600]); print(" got ", gerr or json.dumps(got)[:600])
print(f"seed={seed} cases={checked} mismatches={fails}")
sys.exit(1 if fails else 0)
