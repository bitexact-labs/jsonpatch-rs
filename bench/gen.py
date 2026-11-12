"""Realistic document pairs: API payload snapshots (list of records), k8s-like config, edits."""
import random, copy
def record(r, i):
    return {"id": i, "name": f"user{i}", "email": f"u{i}@example.com", "active": r.random()<0.8,
            "score": round(r.random()*100, 3), "tags": [r.choice("abcdefgh") for _ in range(r.randint(0,4))],
            "address": {"city": r.choice(["Kyiv","Lviv","Berlin","Paris"]), "zip": str(r.randint(10000,99999))},
            "orders": [{"sku": f"S{r.randint(1,999)}", "qty": r.randint(1,5)} for _ in range(r.randint(0,3))]}
def mutate(r, doc, rate):
    d = copy.deepcopy(doc)
    recs = d["items"]
    n = len(recs)
    for _ in range(int(n*rate)):
        k = r.random()
        i = r.randrange(len(recs))
        if k < 0.4: recs[i]["score"] = round(r.random()*100, 3)
        elif k < 0.55: recs[i]["tags"].append("z")
        elif k < 0.7: del recs[i]
        elif k < 0.85: recs.insert(i, record(r, 10**6 + r.randrange(10**6)))
        else: recs.insert(r.randrange(len(recs)), recs.pop(i))
    d["meta"]["version"] += 1
    return d
def pair(n, rate=0.05, seed=1):
    r = random.Random(seed)
    src = {"meta": {"version": 1, "source": "api"}, "items": [record(r, i) for i in range(n)]}
    return src, mutate(r, src, rate)
