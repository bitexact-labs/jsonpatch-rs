//! Rust core of jsonpatch.JsonPatch.from_diff (python-json-patch 1.47, BSD-3-Clause).
//!
//! The diff is a line-by-line port of DiffBuilder: same runs, same moves, same
//! array labels, same order of operations. Values are mirrored into an arena
//! once; `json.dumps(_sorted_members(x))` equality is replaced by interned
//! canonical ids, which give the same equivalence on plain JSON documents.
//! Anything that is not a plain JSON document raises `Unsupported`, and the
//! Python wrapper then runs the original implementation.
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

mod seqmatch;
use seqmatch::{Matcher, NONE};

pyo3::create_exception!(_core, Unsupported, pyo3::exceptions::PyException);

const MAX_DEPTH: usize = 400;
const SMALL_DICT: usize = 12;
const EXACT_MATCH_LIMIT: u128 = 500 * 500;
const STEP: i64 = 1 << 16;
const GAP: i64 = 1 << 32;

/// FxHash (rustc's hasher): fast, non-cryptographic; collisions only cost time
#[derive(Default, Clone, Copy)]
pub struct Fx(u64);
const K: u64 = 0x517cc1b727220a95;
impl Fx {
    #[inline]
    fn add(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(K);
    }
}
impl Hasher for Fx {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut c = bytes.chunks_exact(8);
        for ch in &mut c {
            self.add(u64::from_le_bytes(ch.try_into().unwrap()));
        }
        let r = c.remainder();
        if !r.is_empty() {
            let mut b = [0u8; 8];
            b[..r.len()].copy_from_slice(r);
            self.add(u64::from_le_bytes(b) ^ ((r.len() as u64) << 56));
        }
    }
    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64)
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64)
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i)
    }
    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64)
    }
    #[inline]
    fn finish(&self) -> u64 {
        mix(self.0)
    }
}
pub type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<Fx>>;

#[inline]
fn mix(h: u64) -> u64 {
    // splitmix64 finaliser
    let mut z = h.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

enum Kind {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(u32),
    List(Box<[u32]>),
    /// (members in insertion order, members sorted by key id for lookup)
    Dict(Box<[(u32, u32)]>, Box<[(u32, u32)]>),
}

struct Node {
    kind: Kind,
    /// borrowed: the documents outlive the call and are not changed during it
    obj: *mut ffi::PyObject,
    canon: u32,
}

/// Canonical ids: two values get the same id exactly when
/// json.dumps(_sorted_members(a)) == json.dumps(_sorted_members(b)) for plain JSON.
/// Ids are hash-consed: the hash only picks the bucket, equality is checked
/// structurally against the bucket's representatives, so collisions are harmless.
struct Arena {
    nodes: Vec<Node>,
    strs: Vec<&'static str>,
    str_objs: Vec<*mut ffi::PyObject>,
    /// Python's (cached) str hash -> first str id; collisions chained in str_chain
    str_heads: FxMap<isize, u32>,
    str_chain: Vec<u32>,
    /// hash -> first canon id with that hash; further ones are chained in `chain`
    buckets: FxMap<u64, u32>,
    chain: Vec<u32>,
    /// canon id -> representative node
    reps: Vec<u32>,
    t_list: *mut ffi::PyTypeObject,
    t_dict: *mut ffi::PyTypeObject,
    t_str: *mut ffi::PyTypeObject,
    t_int: *mut ffi::PyTypeObject,
    t_float: *mut ffi::PyTypeObject,
    t_bool: *mut ffi::PyTypeObject,
}

fn unsupported(msg: &str) -> PyErr {
    Unsupported::new_err(msg.to_string())
}

impl Arena {
    fn new() -> Self {
        unsafe {
            Arena {
                nodes: Vec::with_capacity(1024),
                strs: vec![],
                str_objs: vec![],
                str_heads: FxMap::default(),
                str_chain: vec![],
                buckets: FxMap::default(),
                chain: vec![],
                reps: vec![],
                t_list: &raw mut ffi::PyList_Type,
                t_dict: &raw mut ffi::PyDict_Type,
                t_str: &raw mut ffi::PyUnicode_Type,
                t_int: &raw mut ffi::PyLong_Type,
                t_float: &raw mut ffi::PyFloat_Type,
                t_bool: &raw mut ffi::PyBool_Type,
            }
        }
    }

    /// the UTF-8 buffer is cached inside the str object, which outlives the arena
    unsafe fn utf8(obj: *mut ffi::PyObject) -> PyResult<&'static str> {
        let mut size: ffi::Py_ssize_t = 0;
        let p = ffi::PyUnicode_AsUTF8AndSize(obj, &mut size);
        if p.is_null() {
            ffi::PyErr_Clear();
            return Err(unsupported("string with surrogates"));
        }
        let bytes = std::slice::from_raw_parts(p as *const u8, size as usize);
        Ok(std::str::from_utf8_unchecked(bytes))
    }

    unsafe fn intern_str_obj(&mut self, obj: *mut ffi::PyObject) -> PyResult<u32> {
        let h = ffi::PyObject_Hash(obj);
        let head = self.str_heads.get(&h).copied().unwrap_or(u32::MAX);
        let mut c = head;
        let mut text: Option<&'static str> = None;
        while c != u32::MAX {
            if self.str_objs[c as usize] == obj {
                return Ok(c);
            }
            let t = match text {
                Some(t) => t,
                None => {
                    let t = Self::utf8(obj)?;
                    text = Some(t);
                    t
                }
            };
            if self.strs[c as usize] == t {
                return Ok(c);
            }
            c = self.str_chain[c as usize];
        }
        let t = match text {
            Some(t) => t,
            None => Self::utf8(obj)?,
        };
        let id = self.strs.len() as u32;
        self.strs.push(t);
        self.str_objs.push(obj);
        self.str_chain.push(head);
        self.str_heads.insert(h, id);
        Ok(id)
    }

    fn same_canon(&self, a: &Kind, b: &Kind) -> bool {
        match (a, b) {
            (Kind::Null, Kind::Null) => true,
            (Kind::Bool(x), Kind::Bool(y)) => x == y,
            (Kind::Int(x), Kind::Int(y)) => x == y,
            (Kind::Float(x), Kind::Float(y)) => x.to_bits() == y.to_bits(),
            (Kind::Str(x), Kind::Str(y)) => x == y,
            (Kind::List(x), Kind::List(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(&p, &q)| self.nodes[p as usize].canon == self.nodes[q as usize].canon)
            }
            (Kind::Dict(x, _), Kind::Dict(ym, yk)) => {
                x.len() == ym.len()
                    && x.iter().all(|&(k, v)| match self.lookup(ym, yk, k) {
                        Some(w) => self.nodes[v as usize].canon == self.nodes[w as usize].canon,
                        None => false,
                    })
            }
            _ => false,
        }
    }

    fn push(&mut self, kind: Kind, obj: *mut ffi::PyObject, h: u64) -> u32 {
        let id = self.nodes.len() as u32;
        let mut canon = u32::MAX;
        let head = self.buckets.get(&h).copied();
        let mut c = head.unwrap_or(u32::MAX);
        while c != u32::MAX {
            let rep = self.reps[c as usize];
            if self.same_canon(&self.nodes[rep as usize].kind, &kind) {
                canon = c;
                break;
            }
            c = self.chain[c as usize];
        }
        if canon == u32::MAX {
            canon = self.reps.len() as u32;
            self.reps.push(id);
            self.chain.push(head.unwrap_or(u32::MAX));
            self.buckets.insert(h, canon);
        }
        self.nodes.push(Node { kind, obj, canon });
        id
    }

    fn build(&mut self, obj: &Bound<'_, PyAny>, depth: usize) -> PyResult<u32> {
        unsafe { self.build_raw(obj.as_ptr(), depth) }
    }

    unsafe fn build_raw(&mut self, obj: *mut ffi::PyObject, depth: usize) -> PyResult<u32> {
        if depth > MAX_DEPTH {
            return Err(unsupported("document too deep"));
        }
        let t = ffi::Py_TYPE(obj);
        if t == self.t_str {
            let id = self.intern_str_obj(obj)?;
            return Ok(self.push(Kind::Str(id), obj, mix(1 ^ ((id as u64) << 8))));
        }
        if t == self.t_int {
            let mut overflow: std::os::raw::c_int = 0;
            let v = ffi::PyLong_AsLongLongAndOverflow(obj, &mut overflow);
            if overflow != 0 || (v == -1 && !ffi::PyErr_Occurred().is_null()) {
                ffi::PyErr_Clear();
                return Err(unsupported("int out of range"));
            }
            return Ok(self.push(Kind::Int(v), obj, mix(2 ^ mix(v as u64))));
        }
        if t == self.t_float {
            let v = ffi::PyFloat_AsDouble(obj);
            if !v.is_finite() {
                return Err(unsupported("non-finite float"));
            }
            return Ok(self.push(Kind::Float(v), obj, mix(3 ^ mix(v.to_bits()))));
        }
        if t == self.t_bool {
            let b = obj == ffi::Py_True();
            return Ok(self.push(Kind::Bool(b), obj, mix(4 + b as u64)));
        }
        if obj == ffi::Py_None() {
            return Ok(self.push(Kind::Null, obj, mix(6)));
        }
        if t == self.t_list {
            let n = ffi::PyList_GET_SIZE(obj);
            let mut items = Vec::with_capacity(n as usize);
            let mut h = Fx(7);
            let mut i = 0;
            while i < ffi::PyList_GET_SIZE(obj) {
                let it = ffi::PyList_GET_ITEM(obj, i);
                let c = self.build_raw(it, depth + 1)?;
                h.add(self.nodes[c as usize].canon as u64);
                items.push(c);
                i += 1;
            }
            h.add(items.len() as u64);
            return Ok(self.push(Kind::List(items.into_boxed_slice()), obj, mix(h.0)));
        }
        if t == self.t_dict {
            let mut members = Vec::with_capacity(ffi::PyDict_Size(obj) as usize);
            let mut pos: ffi::Py_ssize_t = 0;
            let mut k: *mut ffi::PyObject = std::ptr::null_mut();
            let mut v: *mut ffi::PyObject = std::ptr::null_mut();
            // order-independent: sum of mixed (key, value-canon) pairs
            let mut h: u64 = 8;
            while ffi::PyDict_Next(obj, &mut pos, &mut k, &mut v) != 0 {
                if ffi::Py_TYPE(k) != self.t_str {
                    return Err(unsupported("non-string key"));
                }
                let kid = self.intern_str_obj(k)?;
                let vid = self.build_raw(v, depth + 1)?;
                h = h.wrapping_add(mix(((kid as u64) << 32) ^ self.nodes[vid as usize].canon as u64));
                members.push((kid, vid));
            }
            let by_key = if members.len() > SMALL_DICT {
                let mut v = members.clone();
                v.sort_unstable_by_key(|m| m.0);
                v
            } else {
                Vec::new()
            };
            let n = members.len() as u64;
            return Ok(self.push(Kind::Dict(members.into_boxed_slice(), by_key.into_boxed_slice()), obj, mix(h ^ (n << 40))));
        }
        Err(unsupported("not a plain JSON value"))
    }

    /// member lookup: small dicts are scanned, large ones have a sorted copy
    #[inline]
    fn lookup(&self, members: &[(u32, u32)], by_key: &[(u32, u32)], key: u32) -> Option<u32> {
        if by_key.is_empty() {
            members.iter().find(|m| m.0 == key).map(|m| m.1)
        } else {
            by_key.binary_search_by_key(&key, |m| m.0).ok().map(|i| by_key[i].1)
        }
    }

    /// Python `==` on the mirrored values (no NaN, so no identity shortcut matters)
    fn py_eq(&self, a: u32, b: u32) -> bool {
        let (na, nb) = (&self.nodes[a as usize], &self.nodes[b as usize]);
        if na.canon == nb.canon {
            return true;
        }
        match (&na.kind, &nb.kind) {
            (Kind::List(x), Kind::List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(&p, &q)| self.py_eq(p, q)),
            (Kind::Dict(x, _), Kind::Dict(ym, ykeys)) => {
                x.len() == ym.len()
                    && x.iter().all(|&(k, v)| match self.lookup(ym, ykeys, k) {
                        Some(w) => self.py_eq(v, w),
                        None => false,
                    })
            }
            (ka, kb) => match (num(ka), num(kb)) {
                (Some(p), Some(q)) => num_eq(p, q),
                _ => false,
            },
        }
    }

    fn positional_cost(&self, a: u32, b: u32) -> u128 {
        if self.py_eq(a, b) {
            return 0;
        }
        let (na, nb) = (&self.nodes[a as usize], &self.nodes[b as usize]);
        match (&na.kind, &nb.kind) {
            (Kind::Dict(x, xkeys), Kind::Dict(y, ykeys)) => {
                let mut s = 0u128;
                for &(k, v) in x {
                    s += match self.lookup(y, ykeys, k) {
                        Some(w) => self.positional_cost(v, w),
                        None => 1,
                    };
                }
                for &(k, _) in y {
                    if self.lookup(x, xkeys, k).is_none() {
                        s += 1;
                    }
                }
                s
            }
            (Kind::List(x), Kind::List(y)) => {
                let mut s = (x.len() as i128 - y.len() as i128).unsigned_abs();
                for (&p, &q) in x.iter().zip(y) {
                    s += self.positional_cost(p, q);
                }
                s
            }
            _ => 1,
        }
    }
}

#[derive(Clone, Copy)]
enum Num {
    I(i64),
    F(f64),
}

fn num(k: &Kind) -> Option<Num> {
    match k {
        Kind::Bool(b) => Some(Num::I(*b as i64)),
        Kind::Int(i) => Some(Num::I(*i)),
        Kind::Float(f) => Some(Num::F(*f)),
        _ => None,
    }
}

fn int_float_eq(i: i64, f: f64) -> bool {
    if f.fract() != 0.0 || f.abs() > 1e19 {
        return false;
    }
    (f as i128) == i as i128
}

fn num_eq(a: Num, b: Num) -> bool {
    match (a, b) {
        (Num::I(x), Num::I(y)) => x == y,
        (Num::F(x), Num::F(y)) => x == y,
        (Num::I(x), Num::F(y)) | (Num::F(y), Num::I(x)) => int_float_eq(x, y),
    }
}

// ---------------------------------------------------------------- builder

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Part {
    Key(u32),
    Item(u32),
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum PPart {
    Key(u32),
    Idx(i64),
}

type Loc = Vec<Part>;
type Parts = Vec<PPart>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpKind {
    Add,
    Remove,
    Replace,
    Move,
    Test,
    Append,
}

#[derive(Clone)]
enum Val {
    None,
    Node(u32),
    NewStr(String),
}

struct Op {
    kind: OpKind,
    path: Loc,
    from: Option<Loc>,
    value: Val,
}

struct Slot {
    prev: usize,
    next: usize,
    op: Option<Op>,
}

struct ItemD {
    array: u32,
    label: i64,
    initial: bool,
}

#[derive(Default)]
struct Arr {
    items: Vec<u32>,
    labels: Vec<i64>,
    present: Vec<u32>,
    present_labels: Vec<i64>,
}

fn bisect_left(v: &[i64], x: i64) -> usize {
    v.partition_point(|&y| y < x)
}
fn bisect_right(v: &[i64], x: i64) -> usize {
    v.partition_point(|&y| y <= x)
}

const ST_ADD: usize = 0;
const ST_REMOVE: usize = 1;

struct Builder<'a> {
    a: &'a Arena,
    optimization: bool,
    test_ops: bool,
    append: bool,
    slots: Vec<Slot>,
    remove_tests: HashMap<usize, usize>,
    storage: [HashMap<u32, Vec<usize>>; 2],
    arrays: HashMap<Loc, u32>,
    arrs: Vec<Arr>,
    items: Vec<ItemD>,
}

fn internal(msg: &str) -> PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(format!("jsonpatch_rs internal error: {msg}"))
}

fn join(path: &Parts, key: Option<PPart>) -> Parts {
    let mut p = path.clone();
    if let Some(k) = key {
        p.push(k);
    }
    p
}

fn is_prefix(sub: &[PPart], parts: &[PPart]) -> bool {
    sub.len() <= parts.len() && sub == &parts[..sub.len()]
}

fn is_inside(parts: &[PPart], container: &[PPart]) -> bool {
    parts.len() > container.len() && is_prefix(container, parts)
}

/// _without_item(parts, location)
fn without_item(parts: &Parts, location: &Parts) -> Parts {
    if location.is_empty() {
        // location[-1] raises IndexError in Python; unreachable for array diffs
        return parts.clone();
    }
    let depth = location.len() - 1;
    if let PPart::Idx(last) = location[depth] {
        if is_inside(parts, &location[..depth]) {
            if let PPart::Idx(p) = parts[depth] {
                if p > last {
                    let mut out = parts.clone();
                    out[depth] = PPart::Idx(p - 1);
                    return out;
                }
            } else {
                // str > int raises TypeError in Python
                return parts.clone();
            }
        }
    }
    parts.clone()
}

impl<'a> Builder<'a> {
    fn new(a: &'a Arena, optimization: bool, test_ops: bool, append: bool) -> Self {
        Builder {
            a,
            optimization,
            test_ops,
            append,
            slots: vec![Slot { prev: 0, next: 0, op: None }],
            remove_tests: HashMap::new(),
            storage: [HashMap::new(), HashMap::new()],
            arrays: HashMap::new(),
            arrs: vec![],
            items: vec![],
        }
    }

    fn canon(&self, n: u32) -> u32 {
        self.a.nodes[n as usize].canon
    }

    // --- linked list of operations
    fn insert(&mut self, op: Op) -> usize {
        let last = self.slots[0].prev;
        let id = self.slots.len();
        self.slots.push(Slot { prev: last, next: 0, op: Some(op) });
        self.slots[last].next = id;
        self.slots[0].prev = id;
        id
    }

    fn remove(&mut self, id: usize) {
        let (p, n) = (self.slots[id].prev, self.slots[id].next);
        self.slots[p].next = n;
        self.slots[n].prev = p;
        self.slots[id].prev = id;
        self.slots[id].next = id;
    }

    fn op(&self, id: usize) -> &Op {
        self.slots[id].op.as_ref().unwrap()
    }

    fn store_index(&mut self, value: u32, index: usize, st: usize) {
        if !self.optimization {
            return;
        }
        let k = self.canon(value);
        self.storage[st].entry(k).or_default().push(index);
    }

    fn take_index(&mut self, value: u32, st: usize) -> Option<usize> {
        if !self.optimization {
            return None;
        }
        let k = self.canon(value);
        self.storage[st].get_mut(&k).and_then(|v| v.pop())
    }

    // --- arrays
    fn array_items(&mut self, loc: &Loc) -> u32 {
        if let Some(&a) = self.arrays.get(loc) {
            return a;
        }
        let id = self.arrs.len() as u32;
        self.arrs.push(Arr::default());
        self.arrays.insert(loc.clone(), id);
        id
    }

    fn at(&mut self, arr: u32, index: i64) -> PyResult<u32> {
        if index < 0 {
            return Err(internal("negative index"));
        }
        let index = index as usize;
        loop {
            let a = &self.arrs[arr as usize];
            if a.present.len() > index {
                return Ok(a.present[index]);
            }
            let label = a.labels.last().map(|l| l + GAP).unwrap_or(0);
            let item = self.items.len() as u32;
            self.items.push(ItemD { array: arr, label, initial: true });
            let a = &mut self.arrs[arr as usize];
            a.items.push(item);
            a.labels.push(label);
            a.present.push(item);
            a.present_labels.push(label);
        }
    }

    fn arr_insert(&mut self, arr: u32, index: i64) -> PyResult<u32> {
        let mut position = 0usize;
        if index != 0 {
            let prev = self.at(arr, index - 1)?;
            let pl = self.items[prev as usize].label;
            position = bisect_right(&self.arrs[arr as usize].labels, pl);
        }
        let n = self.arrs[arr as usize].labels.len();
        let label;
        if position == 0 || position == n {
            let a = &self.arrs[arr as usize];
            if a.labels.is_empty() {
                label = 0;
            } else if position == 0 {
                label = a.labels[0] - GAP;
            } else {
                label = a.labels[n - 1] + GAP;
            }
        } else {
            if self.arrs[arr as usize].labels[position] - self.arrs[arr as usize].labels[position - 1] < 2 {
                self.relabel(arr);
            }
            let a = &self.arrs[arr as usize];
            let (low, high) = (a.labels[position - 1], a.labels[position]);
            label = low + STEP.min((high - low).div_euclid(2));
        }
        let item = self.items.len() as u32;
        self.items.push(ItemD { array: arr, label, initial: false });
        let a = &mut self.arrs[arr as usize];
        a.items.insert(position, item);
        a.labels.insert(position, label);
        self.arr_add(item)?;
        Ok(item)
    }

    fn relabel(&mut self, arr: u32) {
        let a = &mut self.arrs[arr as usize];
        a.labels = (0..a.items.len() as i64).map(|i| i * GAP).collect();
        for (k, &it) in a.items.iter().enumerate() {
            self.items[it as usize].label = k as i64 * GAP;
        }
        a.present_labels = a.present.iter().map(|&it| self.items[it as usize].label).collect();
    }

    fn arr_add(&mut self, item: u32) -> PyResult<()> {
        let d = &self.items[item as usize];
        let a = &mut self.arrs[d.array as usize];
        let pos = bisect_left(&a.present_labels, d.label);
        a.present.insert(pos, item);
        a.present_labels.insert(pos, d.label);
        Ok(())
    }

    fn arr_remove(&mut self, item: u32) -> PyResult<()> {
        let d = &self.items[item as usize];
        let a = &mut self.arrs[d.array as usize];
        let pos = bisect_left(&a.present_labels, d.label);
        if pos >= a.present.len() {
            return Err(internal("remove of missing array item"));
        }
        a.present.remove(pos);
        a.present_labels.remove(pos);
        Ok(())
    }

    fn reset(&mut self) {
        for a in self.arrs.iter_mut() {
            a.present = a.items.iter().copied().filter(|&it| self.items[it as usize].initial).collect();
            a.present_labels = a.present.iter().map(|&it| self.items[it as usize].label).collect();
        }
    }

    fn item_index(&self, item: u32) -> i64 {
        let d = &self.items[item as usize];
        bisect_left(&self.arrs[d.array as usize].present_labels, d.label) as i64
    }

    fn location(&mut self, parts: &Parts) -> PyResult<Loc> {
        let mut loc: Loc = Vec::with_capacity(parts.len());
        for p in parts {
            match p {
                PPart::Key(k) => loc.push(Part::Key(*k)),
                PPart::Idx(i) => {
                    let arr = self.array_items(&loc);
                    let it = self.at(arr, *i)?;
                    loc.push(Part::Item(it));
                }
            }
        }
        Ok(loc)
    }

    fn new_location(&mut self, path: &Parts, key: &PPart) -> PyResult<Loc> {
        let mut loc = self.location(path)?;
        match key {
            PPart::Key(k) => loc.push(Part::Key(*k)),
            PPart::Idx(i) => {
                let arr = self.array_items(&loc);
                let it = self.arr_insert(arr, *i)?;
                loc.push(Part::Item(it));
            }
        }
        Ok(loc)
    }

    fn parts(&self, loc: &Loc) -> Parts {
        loc.iter()
            .map(|p| match p {
                Part::Key(k) => PPart::Key(*k),
                Part::Item(it) => PPart::Idx(self.item_index(*it)),
            })
            .collect()
    }

    fn set_present(&mut self, loc: &Loc, present: bool) -> PyResult<()> {
        if let Some(Part::Item(it)) = loc.last() {
            if present {
                self.arr_add(*it)?;
            } else {
                self.arr_remove(*it)?;
            }
        }
        Ok(())
    }

    // --- diff events
    fn item_added(&mut self, path: &Parts, key: PPart, item: u32) -> PyResult<()> {
        let target = join(path, Some(key.clone()));
        if let Some(index) = self.take_index(item, ST_REMOVE) {
            let removed = self.op(index).path.clone();
            let source = self.parts(&removed);
            if !is_inside(&target, &source) {
                if let Some(t) = self.remove_tests.remove(&index) {
                    self.remove(t);
                }
                self.remove(index);
                if source != target {
                    let path = self.new_location(path, &key)?;
                    self.insert(Op { kind: OpKind::Move, path, from: Some(removed), value: Val::None });
                } else {
                    self.set_present(&removed, true)?;
                }
                return Ok(());
            }
        }
        let loc = self.new_location(path, &key)?;
        let new_index = self.insert(Op { kind: OpKind::Add, path: loc, from: None, value: Val::Node(item) });
        self.store_index(item, new_index, ST_ADD);
        Ok(())
    }

    fn item_removed(&mut self, path: &Parts, key: PPart, item: u32) -> PyResult<()> {
        let source = join(path, Some(key));
        let location = self.location(&source)?;
        if let Some(index) = self.take_index(item, ST_ADD) {
            let added_location = self.op(index).path.clone();
            let added = self.parts(&added_location);
            self.remove(index);
            if without_item(&source, &added) != added {
                self.set_present(&location, false)?;
                self.insert(Op { kind: OpKind::Move, path: added_location, from: Some(location), value: Val::None });
            } else {
                self.set_present(&added_location, false)?;
            }
            return Ok(());
        }
        let test_index = self.insert_test(&location, item);
        self.set_present(&location, false)?;
        let new_index = self.insert(Op { kind: OpKind::Remove, path: location, from: None, value: Val::None });
        if let Some(t) = test_index {
            self.remove_tests.insert(new_index, t);
        }
        self.store_index(item, new_index, ST_REMOVE);
        Ok(())
    }

    fn insert_test(&mut self, loc: &Loc, value: u32) -> Option<usize> {
        if self.test_ops {
            Some(self.insert(Op { kind: OpKind::Test, path: loc.clone(), from: None, value: Val::Node(value) }))
        } else {
            None
        }
    }

    fn item_replaced(&mut self, path: &Parts, key: Option<PPart>, item: u32, old: u32) -> PyResult<()> {
        if self.append && (!path.is_empty() || key.is_some()) {
            if let (Kind::Str(n), Kind::Str(o)) = (&self.a.nodes[item as usize].kind, &self.a.nodes[old as usize].kind) {
                let (ns, os) = (&self.a.strs[*n as usize], &self.a.strs[*o as usize]);
                if ns.chars().count() > os.chars().count() && ns.starts_with(*os) {
                    let suffix = ns[os.len()..].to_string();
                    let loc = self.location(&join(path, key))?;
                    self.insert_test(&loc, old);
                    self.insert(Op { kind: OpKind::Append, path: loc, from: None, value: Val::NewStr(suffix) });
                    return Ok(());
                }
            }
        }
        let loc = self.location(&join(path, key))?;
        self.insert_test(&loc, old);
        self.insert(Op { kind: OpKind::Replace, path: loc, from: None, value: Val::Node(item) });
        Ok(())
    }

    fn compare_dicts(&mut self, path: &Parts, src: u32, dst: u32) -> PyResult<()> {
        let a = self.a;
        let (sm, sk) = match &a.nodes[src as usize].kind {
            Kind::Dict(m, k) => (m, k),
            _ => unreachable!(),
        };
        let (dm, dk) = match &a.nodes[dst as usize].kind {
            Kind::Dict(m, k) => (m, k),
            _ => unreachable!(),
        };
        for &(k, v) in sm {
            if a.lookup(dm, dk, k).is_none() {
                self.item_removed(path, PPart::Key(k), v)?;
            }
        }
        for &(k, v) in dm {
            if a.lookup(sm, sk, k).is_none() {
                self.item_added(path, PPart::Key(k), v)?;
            }
        }
        for &(k, v) in sm {
            if let Some(w) = a.lookup(dm, dk, k) {
                self.compare_values(path, Some(PPart::Key(k)), v, w)?;
            }
        }
        Ok(())
    }

    fn differing_runs(&self, src: &[u32], dst: &[u32]) -> Vec<(usize, usize, usize, usize)> {
        let sk: Vec<u32> = src.iter().map(|&n| self.canon(n)).collect();
        let dk: Vec<u32> = dst.iter().map(|&n| self.canon(n)).collect();
        let shorter = src.len().min(dst.len());
        let mut start = 0;
        while start < shorter && sk[start] == dk[start] {
            start += 1;
        }
        let mut common_end = 0;
        while common_end < shorter - start && sk[src.len() - 1 - common_end] == dk[dst.len() - 1 - common_end] {
            common_end += 1;
        }
        let (src_end, dst_end) = (src.len() - common_end, dst.len() - common_end);
        let cost = |i1: usize, i2: usize, j1: usize, j2: usize| -> u128 {
            let mut c = ((i2 - i1) as i128 - (j2 - j1) as i128).unsigned_abs();
            for (i, j) in (i1..i2).zip(j1..j2) {
                if sk[i] != dk[j] {
                    c += self.a.positional_cost(src[i], dst[j]);
                }
            }
            c
        };
        let by_position = (start, src_end, start, dst_end);
        let position_cost = cost(start, src_end, start, dst_end);
        let exact = ((src_end - start) as u128) * ((dst_end - start) as u128) <= EXACT_MATCH_LIMIT;
        if !exact {
            let mut counts: HashMap<u32, u128> = HashMap::new();
            for &k in &dk[start..dst_end] {
                *counts.entry(k).or_default() += 1;
            }
            let comparisons: u128 = sk[start..src_end].iter().map(|k| counts.get(k).copied().unwrap_or(0)).sum();
            if comparisons > EXACT_MATCH_LIMIT.max(position_cost.saturating_mul(position_cost)) {
                return vec![by_position];
            }
        }
        // local ids for SequenceMatcher
        let mut local: HashMap<u32, u32> = HashMap::new();
        let b: Vec<u32> = dk[start..dst_end]
            .iter()
            .map(|k| {
                let n = local.len() as u32;
                *local.entry(*k).or_insert(n)
            })
            .collect();
        let a: Vec<u32> = sk[start..src_end].iter().map(|k| local.get(k).copied().unwrap_or(NONE)).collect();
        let nu = local.len();
        let mut m = Matcher::new();
        m.set_a(a);
        m.set_b(b, nu, vec![false; nu], !exact);
        let runs: Vec<(usize, usize, usize, usize)> = m
            .opcodes()
            .into_iter()
            .filter(|op| op.0 != "equal")
            .map(|(_, i1, i2, j1, j2)| (start + i1, start + i2, start + j1, start + j2))
            .collect();
        let total: u128 = runs.iter().map(|&(i1, i2, j1, j2)| cost(i1, i2, j1, j2)).sum();
        if total > position_cost {
            return vec![by_position];
        }
        runs
    }

    fn compare_lists(&mut self, path: &Parts, src: u32, dst: u32) -> PyResult<()> {
        let a = self.a;
        let s = match &a.nodes[src as usize].kind {
            Kind::List(v) => v,
            _ => unreachable!(),
        };
        let d = match &a.nodes[dst as usize].kind {
            Kind::List(v) => v,
            _ => unreachable!(),
        };
        for (i1, i2, j1, j2) in self.differing_runs(s, d) {
            let paired = (i2 - i1).min(j2 - j1);
            for key in j1..j1 + paired {
                let (old, new) = (s[i1 + key - j1], d[key]);
                let k = PPart::Idx(key as i64);
                match (&a.nodes[old as usize].kind, &a.nodes[new as usize].kind) {
                    (Kind::Dict(..), Kind::Dict(..)) => self.compare_dicts(&join(path, Some(k)), old, new)?,
                    (Kind::List(_), Kind::List(_)) => self.compare_lists(&join(path, Some(k)), old, new)?,
                    _ => {
                        if self.canon(old) == self.canon(new) {
                            continue;
                        }
                        self.item_removed(path, k.clone(), old)?;
                        self.item_added(path, k, new)?;
                    }
                }
            }
            for &old in &s[i1 + paired..i2] {
                self.item_removed(path, PPart::Idx((j1 + paired) as i64), old)?;
            }
            for key in j1 + paired..j2 {
                self.item_added(path, PPart::Idx(key as i64), d[key])?;
            }
        }
        Ok(())
    }

    fn compare_values(&mut self, path: &Parts, key: Option<PPart>, src: u32, dst: u32) -> PyResult<()> {
        let a = self.a;
        match (&a.nodes[src as usize].kind, &a.nodes[dst as usize].kind) {
            (Kind::Dict(..), Kind::Dict(..)) => self.compare_dicts(&join(path, key), src, dst),
            (Kind::List(_), Kind::List(_)) => self.compare_lists(&join(path, key), src, dst),
            _ => {
                if self.canon(src) == self.canon(dst) {
                    Ok(())
                } else {
                    self.item_replaced(path, key, dst, src)
                }
            }
        }
    }

    // --- output
    fn replay(&mut self) -> PyResult<Vec<(OpKind, Parts, Option<Parts>, Val)>> {
        self.reset();
        let mut out = vec![];
        let mut cur = self.slots[0].next;
        while cur != 0 {
            let (kind, path, from, value) = {
                let op = self.op(cur);
                (op.kind, op.path.clone(), op.from.clone(), op.value.clone())
            };
            let from_parts = match &from {
                Some(f) => {
                    let p = self.parts(f);
                    self.set_present(f, false)?;
                    Some(p)
                }
                None => None,
            };
            let path_parts = self.parts(&path);
            match kind {
                OpKind::Remove => self.set_present(&path, false)?,
                OpKind::Add | OpKind::Move => self.set_present(&path, true)?,
                _ => {}
            }
            out.push((kind, path_parts, from_parts, value));
            cur = self.slots[cur].next;
        }
        Ok(out)
    }
}

fn pointer(a: &Arena, parts: &Parts) -> String {
    let mut s = String::new();
    for p in parts {
        s.push('/');
        match p {
            PPart::Key(k) => {
                for ch in a.strs[*k as usize].chars() {
                    match ch {
                        '~' => s.push_str("~0"),
                        '/' => s.push_str("~1"),
                        c => s.push(c),
                    }
                }
            }
            PPart::Idx(i) => s.push_str(&i.to_string()),
        }
    }
    s
}

/// copy.deepcopy restricted to plain JSON values, sharing one memo like deepcopy(list_of_ops)
fn deepcopy<'py>(py: Python<'py>, obj: &Bound<'py, PyAny>, memo: &mut HashMap<usize, Py<PyAny>>) -> PyResult<Bound<'py, PyAny>> {
    if obj.is_exact_instance_of::<PyList>() {
        let key = obj.as_ptr() as usize;
        if let Some(v) = memo.get(&key) {
            return Ok(v.bind(py).clone());
        }
        let l = obj.cast::<PyList>()?;
        let out = PyList::empty(py);
        memo.insert(key, out.clone().into_any().unbind());
        for it in l.iter() {
            out.append(deepcopy(py, &it, memo)?)?;
        }
        Ok(out.into_any())
    } else if obj.is_exact_instance_of::<PyDict>() {
        let key = obj.as_ptr() as usize;
        if let Some(v) = memo.get(&key) {
            return Ok(v.bind(py).clone());
        }
        let d = obj.cast::<PyDict>()?;
        let out = PyDict::new(py);
        memo.insert(key, out.clone().into_any().unbind());
        for (k, v) in d.iter() {
            out.set_item(k, deepcopy(py, &v, memo)?)?;
        }
        Ok(out.into_any())
    } else {
        Ok(obj.clone())
    }
}

/// diff(src, dst, optimization, generate_test_ops, append) -> list of operation dicts
/// (already deep-copied, exactly what DiffBuilder.execute + copy.deepcopy produce)
#[pyfunction]
#[pyo3(signature = (src, dst, optimization=true, generate_test_ops=false, append=false))]
fn diff<'py>(
    py: Python<'py>,
    src: &Bound<'py, PyAny>,
    dst: &Bound<'py, PyAny>,
    optimization: bool,
    generate_test_ops: bool,
    append: bool,
) -> PyResult<Bound<'py, PyList>> {
    let timing = std::env::var_os("JSONPATCH_RS_TIMING").is_some();
    let t0 = std::time::Instant::now();
    let mut arena = Arena::new();
    let s = arena.build(src, 0)?;
    let d = arena.build(dst, 0)?;
    let t1 = t0.elapsed();
    let mut b = Builder::new(&arena, optimization, generate_test_ops, append);
    b.compare_values(&vec![], None, s, d)?;
    let t2 = t0.elapsed();
    let ops = b.replay()?;
    if timing {
        eprintln!("build {:?} compare {:?} replay {:?}", t1, t2 - t1, t0.elapsed() - t2);
    }

    let out = PyList::empty(py);
    let mut memo: HashMap<usize, Py<PyAny>> = HashMap::new();
    let k_op = PyString::intern(py, "op");
    let k_path = PyString::intern(py, "path");
    let k_from = PyString::intern(py, "from");
    let k_value = PyString::intern(py, "value");
    let mut val = |v: &Val, memo: &mut HashMap<usize, Py<PyAny>>| -> PyResult<Bound<'py, PyAny>> {
        match v {
            Val::Node(n) => deepcopy(py, unsafe { &Bound::from_borrowed_ptr(py, arena.nodes[*n as usize].obj) }, memo),
            Val::NewStr(s) => Ok(PyString::new(py, s).into_any()),
            Val::None => Ok(py.None().into_bound(py)),
        }
    };
    let mut i = 0;
    while i < ops.len() {
        if i + 1 < ops.len() {
            let (f, s2) = (&ops[i], &ops[i + 1]);
            if f.1 == s2.1 && f.0 == OpKind::Remove && s2.0 == OpKind::Add {
                let dct = PyDict::new(py);
                dct.set_item(&k_op, "replace")?;
                dct.set_item(&k_path, pointer(&arena, &s2.1))?;
                dct.set_item(&k_value, val(&s2.3, &mut memo)?)?;
                out.append(dct)?;
                i += 2;
                continue;
            }
        }
        let (kind, path, from, value) = &ops[i];
        let dct = PyDict::new(py);
        let name = match kind {
            OpKind::Add => "add",
            OpKind::Remove => "remove",
            OpKind::Replace => "replace",
            OpKind::Move => "move",
            OpKind::Test => "test",
            OpKind::Append => "append",
        };
        dct.set_item(&k_op, name)?;
        if let Some(f) = from {
            dct.set_item(&k_from, pointer(&arena, f))?;
        }
        dct.set_item(&k_path, pointer(&arena, path))?;
        if !matches!(value, Val::None) {
            dct.set_item(&k_value, val(value, &mut memo)?)?;
        }
        out.append(dct)?;
        i += 1;
    }
    Ok(out)
}

/// copy.deepcopy for plain JSON documents (raises Unsupported otherwise)
#[pyfunction]
fn json_deepcopy<'py>(py: Python<'py>, obj: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    check_plain(obj, 0)?;
    let mut memo = HashMap::new();
    deepcopy(py, obj, &mut memo)
}

fn check_plain(obj: &Bound<'_, PyAny>, depth: usize) -> PyResult<()> {
    if depth > MAX_DEPTH {
        return Err(unsupported("document too deep"));
    }
    if obj.is_none()
        || obj.is_exact_instance_of::<PyBool>()
        || obj.is_exact_instance_of::<PyInt>()
        || obj.is_exact_instance_of::<PyFloat>()
        || obj.is_exact_instance_of::<PyString>()
    {
        return Ok(());
    }
    if obj.is_exact_instance_of::<PyList>() {
        for it in obj.cast::<PyList>()?.iter() {
            check_plain(&it, depth + 1)?;
        }
        return Ok(());
    }
    if obj.is_exact_instance_of::<PyDict>() {
        for (k, v) in obj.cast::<PyDict>()?.iter() {
            check_plain(&k, depth + 1)?;
            check_plain(&v, depth + 1)?;
        }
        return Ok(());
    }
    Err(unsupported("not a plain JSON value"))
}

#[pyfunction]
fn _build_only(src: &Bound<'_, PyAny>) -> PyResult<(usize, f64, f64, f64)> {
    let t0 = std::time::Instant::now();
    let mut arena = Arena::new();
    let t1 = t0.elapsed().as_secs_f64();
    arena.build(src, 0)?;
    let t2 = t0.elapsed().as_secs_f64();
    let n = arena.nodes.len();
    drop(arena);
    Ok((n, t1, t2 - t1, t0.elapsed().as_secs_f64() - t2))
}

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(diff, m)?)?;
    m.add_function(wrap_pyfunction!(json_deepcopy, m)?)?;
    m.add_function(wrap_pyfunction!(_build_only, m)?)?;
    m.add("Unsupported", m.py().get_type::<Unsupported>())?;
    Ok(())
}
