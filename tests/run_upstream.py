"""Run python-json-patch's own test suite (tests.py from the 1.47 sdist) with jsonpatch_rs installed
into the jsonpatch module, and once without, and compare."""
import sys, unittest, os, io
mode = sys.argv[1] if len(sys.argv) > 1 else "rs"
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "upstream"))
import jsonpatch_rs
if mode == "rs":
    jsonpatch_rs.install()
import jsonpatch
calls = {"n": 0}
if mode == "rs":
    orig = jsonpatch_rs._core.diff
    def counted(*a, **k):
        calls["n"] += 1
        return orig(*a, **k)
    jsonpatch_rs._core.diff = counted
if mode == "rs":
    # doctest only collects functions defined in the module itself; the docstrings
    # (and so the doctests) of the replaced functions are the originals
    for f in (jsonpatch_rs._from_diff, jsonpatch_rs.make_patch, jsonpatch_rs.apply_patch):
        f.__module__ = "jsonpatch"
import tests
suite = unittest.defaultTestLoader.loadTestsFromModule(tests)
r = unittest.TextTestRunner(stream=io.StringIO(), verbosity=0).run(suite)
print(f"mode={mode} ran={r.testsRun} failures={len(r.failures)} errors={len(r.errors)} skipped={len(r.skipped)} rust_diff_calls={calls['n']}")
for t, tb in (r.failures + r.errors)[:5]:
    print("FAIL", t, tb[-600:])
