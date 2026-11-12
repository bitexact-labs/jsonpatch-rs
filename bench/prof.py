import sys, time, cProfile, pstats
sys.path.insert(0, sys.argv[1]); sys.path.insert(0, 'bench')
import jsonpatch, gen
for n in (1000, 5000):
    src, dst = gen.pair(n)
    t=time.perf_counter(); p = jsonpatch.make_patch(src, dst); t1=time.perf_counter()-t
    t=time.perf_counter(); out = p.apply(src); t2=time.perf_counter()-t
    assert out == dst
    print(n, len(p.patch), 'ops make %.3fs apply %.3fs' % (t1, t2))
src, dst = gen.pair(5000)
cProfile.run('jsonpatch.make_patch(src, dst)', '/tmp/mp.prof')
pstats.Stats('/tmp/mp.prof').sort_stats('tottime').print_stats(12)
