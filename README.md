# jsonpatch-rs: `jsonpatch.make_patch` in Rust, the same patch 6–10× faster

A drop-in accelerator for [python-json-patch](https://github.com/stefankoegl/python-json-patch) (`jsonpatch`, ~100M downloads/month).
`make_patch` / `JsonPatch.from_diff` run a line-by-line Rust port of jsonpatch's `DiffBuilder` and return **the identical patch**:
same operations, same order, same `move` detection, same array indices. `apply_patch` gets a Rust deep copy.

```python
import jsonpatch_rs as jsonpatch            # same API as jsonpatch
patch = jsonpatch.make_patch(old_doc, new_doc)

# or speed up code that already does `import jsonpatch`:
import jsonpatch_rs; jsonpatch_rs.install()
```

## Benchmark (Intel i5-8400T, Python 3.14, jsonpatch 1.47)
| Input | ops | make_patch: jsonpatch | jsonpatch-rs | speedup | apply_patch: jsonpatch | jsonpatch-rs | speedup |
|---|---:|---:|---:|---:|---:|---:|---:|
| config, 1 manifest (~60 KB) | 6 | 2.5 ms | 0.3 ms | **9×** | 0.7 ms | 0.2 ms | 3.1× |
| API snapshot, 1,000 records (0.2 MB), 5% changed | 61 | 44.8 ms | 4.4 ms | **10×** | 8.1 ms | 2.0 ms | 4.0× |
| API snapshot, 10,000 records (2.2 MB), 5% changed | 633 | 486.5 ms | 73.6 ms | **7×** | 85.4 ms | 22.5 ms | 3.8× |
| API snapshot, 50,000 records (10.9 MB), 5% changed | 3205 | 2,607.4 ms | 453.5 ms | **6×** | 465.4 ms | 126.4 ms | 3.7× |

Reproduce: `python bench/bench.py` (asserts that both libraries return equal patches and equal patched documents).

## How it is verified
- **Upstream test suite:** jsonpatch's own `tests.py` (353 tests incl. doctests) passes with `install()`; the Rust diff is hit 162 times. `python tests/run_upstream.py rs`.
- **Differential fuzzing:** `tests/fuzz_vs_orig.py` generates random nested documents (keys with `~` and `/`, Unicode, `1` vs `True` vs `1.0` vs `-0.0`,
  duplicates that trigger moves, lists of 400–900 items that switch SequenceMatcher to autojunk mode) and random edits, with
  `generate_test_ops`, `optimization=False` and `JsonPatchWithAppend`. 200,000 cases on the final build, **0 differences**
  (compared type-exactly, values checked to be copies). Logs: `tests/fuzz_*.log`.
- **Fallback:** `tests/test_fallback.py` — tuples, NaN, ints beyond 64 bit, non-string keys, dict subclasses, lone surrogates,
  1000-deep nesting, custom `dumps`: all give exactly what jsonpatch gives.

## Where it is not faster / limits
- The fast path covers plain JSON documents: `dict`/`list`/`str`/`int` (64-bit)/`float` (finite)/`bool`/`None`, string keys,
  nesting up to 400. Anything else, a custom `dumps`, or a pointer class with its own path syntax runs the original Python code
  (correct, but not faster).
- Most of the remaining time is reading the Python objects (~150–200 ns per value), so the speedup drops to ~6× on very large documents.
  The docs are mirrored in memory during the diff (about 40 bytes per value).
- Tied to jsonpatch 1.47's algorithm (pinned in the dependencies). With any other jsonpatch version installed, the fast path switches itself off and jsonpatch's own code runs.
- Applying patches is still jsonpatch's code; only the deep copy is native (3–4×).

## Competitors
We found no Rust- or C-backed RFC 6902 *diff* for Python on PyPI that reproduces jsonpatch's output (Nov 2026 search:
json-tools-rs does flatten/unflatten, jsonpatchx and others are pure Python or apply-only). If you know one, open an issue and we will benchmark against it.

## Build
```
pip install maturin
maturin develop --release
```

## License
BSD-3-Clause, see `LICENSE` and `NOTICE`. Derived from python-json-patch (Stefan Kögl); `src/seqmatch.rs` from CPython's difflib (PSF-2.0).
