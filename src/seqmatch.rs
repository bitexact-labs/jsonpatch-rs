//! Port of CPython's difflib.SequenceMatcher (Lib/difflib.py, PSF-2.0) working on integer ids.
//! Elements are interned to u32 ids by the caller; `NONE` marks an element of `a` that does not occur in `b`.

pub const NONE: u32 = u32::MAX;


#[derive(Default)]
pub struct Matcher {
    a: Vec<u32>,
    b: Vec<u32>,
    b2j: Vec<Vec<u32>>,
    bjunk: Vec<bool>,
    n_unique: usize,
    // j2len emulation: two rows with stamps, see find_longest_match
    rows: [Vec<(u64, u32)>; 2],
    tick: u64,
    blocks: Option<Vec<(usize, usize, usize)>>,
    fullbcount: Option<Vec<u32>>,
}

fn calc_ratio(matches: usize, length: usize) -> f64 {
    if length > 0 {
        2.0 * matches as f64 / length as f64
    } else {
        1.0
    }
}

impl Matcher {
    pub fn new() -> Self {
        Matcher::default()
    }

    pub fn len_a(&self) -> usize {
        self.a.len()
    }
    pub fn len_b(&self) -> usize {
        self.b.len()
    }

    pub fn set_a(&mut self, a: Vec<u32>) {
        self.a = a;
        self.blocks = None;
    }

    /// `junk[id]` = element id is junk. Builds b2j like __chain_b (junk purged, then popular purged).
    pub fn set_b(&mut self, b: Vec<u32>, n_unique: usize, junk: Vec<bool>, autojunk: bool) {
        let mut b2j: Vec<Vec<u32>> = vec![Vec::new(); n_unique];
        for (i, &id) in b.iter().enumerate() {
            b2j[id as usize].push(i as u32);
        }
        for (id, &is_junk) in junk.iter().enumerate() {
            if is_junk {
                b2j[id].clear();
            }
        }
        let n = b.len();
        if autojunk && n >= 200 {
            let ntest = n / 100 + 1;
            for v in b2j.iter_mut() {
                if v.len() > ntest {
                    v.clear();
                }
            }
        }
        self.rows = [vec![(0, 0); n + 1], vec![(0, 0); n + 1]];
        self.b = b;
        self.b2j = b2j;
        self.bjunk = junk;
        self.n_unique = n_unique;
        self.blocks = None;
        self.fullbcount = None;
    }

    fn isbjunk(&self, id: u32) -> bool {
        (id as usize) < self.bjunk.len() && self.bjunk[id as usize]
    }

    pub fn find_longest_match(&mut self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        self.tick += 2; // so that tick-1 is never a stamp written by an earlier call
        for i in alo..ahi {
            self.tick += 1;
            let tick = self.tick;
            let id = self.a[i];
            if id == NONE {
                continue;
            }
            let cur = (tick & 1) as usize;
            let prev = cur ^ 1;
            let list = &self.b2j[id as usize];
            let start = list.partition_point(|&j| (j as usize) < blo);
            for &j in &list[start..] {
                let j = j as usize;
                if j >= bhi {
                    break;
                }
                let (stamp, val) = self.rows[prev][j]; // index j == (j-1)+1
                let k = if stamp == tick - 1 { val + 1 } else { 1 };
                self.rows[cur][j + 1] = (tick, k);
                if k as usize > bestsize {
                    besti = i + 1 - k as usize;
                    bestj = j + 1 - k as usize;
                    bestsize = k as usize;
                }
            }
        }
        while besti > alo && bestj > blo && !self.isbjunk(self.b[bestj - 1]) && self.a[besti - 1] == self.b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && !self.isbjunk(self.b[bestj + bestsize])
            && self.a[besti + bestsize] == self.b[bestj + bestsize]
        {
            bestsize += 1;
        }
        while besti > alo && bestj > blo && self.isbjunk(self.b[bestj - 1]) && self.a[besti - 1] == self.b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && self.isbjunk(self.b[bestj + bestsize])
            && self.a[besti + bestsize] == self.b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    pub fn matching_blocks(&mut self) -> Vec<(usize, usize, usize)> {
        if let Some(b) = &self.blocks {
            return b.clone();
        }
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0usize, la, 0usize, lb)];
        let mut mb: Vec<(usize, usize, usize)> = vec![];
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                mb.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        mb.sort();
        let (mut i1, mut j1, mut k1) = (0usize, 0usize, 0usize);
        let mut non_adjacent = vec![];
        for (i2, j2, k2) in mb {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    non_adjacent.push((i1, j1, k1));
                }
                i1 = i2;
                j1 = j2;
                k1 = k2;
            }
        }
        if k1 > 0 {
            non_adjacent.push((i1, j1, k1));
        }
        non_adjacent.push((la, lb, 0));
        self.blocks = Some(non_adjacent.clone());
        non_adjacent
    }

    pub fn opcodes(&mut self) -> Vec<(&'static str, usize, usize, usize, usize)> {
        let (mut i, mut j) = (0usize, 0usize);
        let mut answer = vec![];
        for (ai, bj, size) in self.matching_blocks() {
            let tag = if i < ai && j < bj {
                "replace"
            } else if i < ai {
                "delete"
            } else if j < bj {
                "insert"
            } else {
                ""
            };
            if !tag.is_empty() {
                answer.push((tag, i, ai, j, bj));
            }
            i = ai + size;
            j = bj + size;
            if size > 0 {
                answer.push(("equal", ai, i, bj, j));
            }
        }
        answer
    }

    pub fn grouped_opcodes(&mut self, n: usize) -> Vec<Vec<(&'static str, usize, usize, usize, usize)>> {
        let mut codes = self.opcodes();
        if codes.is_empty() {
            codes = vec![("equal", 0, 1, 0, 1)];
        }
        if codes[0].0 == "equal" {
            let (tag, i1, i2, j1, j2) = codes[0];
            codes[0] = (tag, i1.max(i2.saturating_sub(n)), i2, j1.max(j2.saturating_sub(n)), j2);
        }
        let last = codes.len() - 1;
        if codes[last].0 == "equal" {
            let (tag, i1, i2, j1, j2) = codes[last];
            codes[last] = (tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n));
        }
        let nn = n + n;
        let mut groups = vec![];
        let mut group: Vec<(&'static str, usize, usize, usize, usize)> = vec![];
        for (tag, mut i1, i2, mut j1, j2) in codes {
            if tag == "equal" && i2 - i1 > nn {
                group.push((tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n)));
                groups.push(std::mem::take(&mut group));
                i1 = i1.max(i2.saturating_sub(n));
                j1 = j1.max(j2.saturating_sub(n));
            }
            group.push((tag, i1, i2, j1, j2));
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].0 == "equal") {
            groups.push(group);
        }
        groups
    }

    pub fn ratio(&mut self) -> f64 {
        let matches: usize = self.matching_blocks().iter().map(|t| t.2).sum();
        calc_ratio(matches, self.a.len() + self.b.len())
    }

    pub fn quick_ratio(&mut self) -> f64 {
        if self.fullbcount.is_none() {
            let mut c = vec![0u32; self.n_unique];
            for &id in &self.b {
                c[id as usize] += 1;
            }
            self.fullbcount = Some(c);
        }
        let full = self.fullbcount.as_ref().unwrap();
        let mut avail: Vec<i64> = full.iter().map(|&x| x as i64).collect();
        let mut matches = 0usize;
        for &id in &self.a {
            if id == NONE {
                continue;
            }
            let numb = avail[id as usize];
            avail[id as usize] = numb - 1;
            if numb > 0 {
                matches += 1;
            }
        }
        calc_ratio(matches, self.a.len() + self.b.len())
    }

    pub fn real_quick_ratio(&self) -> f64 {
        let (la, lb) = (self.a.len(), self.b.len());
        calc_ratio(la.min(lb), la + lb)
    }
}
