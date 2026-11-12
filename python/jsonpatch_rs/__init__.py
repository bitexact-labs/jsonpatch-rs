"""Drop-in speedup for python-json-patch (``jsonpatch``).

``make_patch`` / ``JsonPatch.from_diff`` run a Rust port of jsonpatch's
DiffBuilder and return the very same patch. ``apply_patch`` uses a Rust
deep copy of the document. Everything else is the original ``jsonpatch``.

Inputs that are not plain JSON documents (dict/list/str/int/float/bool/None
with str keys, finite floats, 64-bit ints), custom ``dumps``, or pointer
classes with their own path syntax fall back to the original code, so the
result is always what ``jsonpatch`` itself returns.

Usage::

    import jsonpatch_rs as jsonpatch          # same API
    # or, to speed up code that already imports jsonpatch:
    import jsonpatch_rs; jsonpatch_rs.install()
"""
import copy as _copy
import json as _json

import jsonpatch as _jp
from jsonpatch import *  # noqa: F401,F403  re-export the whole public API
from jsonpatch import JsonPointer, JsonPatch, JsonPatchWithAppend

from . import _core

__all__ = [name for name in dir(_jp) if not name.startswith('_')]
__version__ = '0.1.0'
jsonpatch_version = _jp.__version__

_orig_from_diff = JsonPatch.__dict__['from_diff'].__func__
_orig_from_diff_append = JsonPatchWithAppend.__dict__['from_diff'].__func__
_orig_apply = JsonPatch.apply
_Unsupported = _core.Unsupported


def _default_paths(pointer_cls):
    from_parts = getattr(pointer_cls, 'from_parts', None)
    return from_parts is None or \
        getattr(from_parts, '__func__', None) is _jp._JSONPOINTER_FROM_PARTS


# the Rust diff is a port of this exact jsonpatch version; with another one, use its own code
PORTED_VERSION = '1.47'
_FAST = _jp.__version__ == PORTED_VERSION


def _fast_ops(cls, src, dst, optimization, dumps, pointer_cls, generate_test_ops, append):
    if not _FAST:
        return None
    dumper = dumps or cls.json_dumper
    if dumper is not _json.dumps or not _default_paths(pointer_cls):
        return None
    try:
        return _core.diff(src, dst, bool(optimization), bool(generate_test_ops), append)
    except _Unsupported:
        return None


def _from_diff(cls, src, dst, optimization=True, dumps=None,
               pointer_cls=JsonPointer, generate_test_ops=False):
    append = issubclass(cls, JsonPatchWithAppend)
    ops = _fast_ops(cls, src, dst, optimization, dumps, pointer_cls, generate_test_ops, append)
    if ops is None:
        orig = _orig_from_diff_append if append else _orig_from_diff
        return orig(cls, src, dst, optimization=optimization, dumps=dumps,
                    pointer_cls=pointer_cls, generate_test_ops=generate_test_ops)
    return cls(ops, pointer_cls=pointer_cls)


def make_patch(src, dst, pointer_cls=JsonPointer, generate_test_ops=False):
    """Same as jsonpatch.make_patch, returns a jsonpatch.JsonPatch."""
    return _from_diff(JsonPatch, src, dst, pointer_cls=pointer_cls,
                      generate_test_ops=generate_test_ops)


def _deepcopy(obj):
    try:
        return _core.json_deepcopy(obj)
    except _Unsupported:
        return _copy.deepcopy(obj)


def _apply(self, obj, in_place=False, atomic=False):
    if not in_place:
        # same as the original, which deep-copies and then never uses atomic
        return _orig_apply(self, _deepcopy(obj), in_place=True, atomic=False)
    return _orig_apply(self, obj, in_place=in_place, atomic=atomic)


def apply_patch(doc, patch, in_place=False, pointer_cls=JsonPointer, atomic=False):
    """Same as jsonpatch.apply_patch."""
    if isinstance(patch, (str, bytes)):
        patch = JsonPatch.from_string(patch, pointer_cls=pointer_cls)
    else:
        patch = JsonPatch(patch, pointer_cls=pointer_cls)
    return _apply(patch, doc, in_place, atomic=atomic)


def diff_ops(src, dst, optimization=True, generate_test_ops=False):
    """The operations as a list of dicts, without building JsonPatch objects
    (what ``list(make_patch(src, dst))`` returns)."""
    ops = _fast_ops(JsonPatch, src, dst, optimization, None, JsonPointer, generate_test_ops, False)
    if ops is None:
        ops = list(_orig_from_diff(JsonPatch, src, dst, optimization=optimization,
                                   generate_test_ops=generate_test_ops))
    return ops


_installed = False


def install():
    """Patch the ``jsonpatch`` module in place, so existing code gets faster."""
    global _installed
    if _installed:
        return
    JsonPatch.from_diff = classmethod(_from_diff)
    JsonPatchWithAppend.from_diff = classmethod(_from_diff)
    JsonPatch.apply = _apply
    _jp.make_patch = make_patch
    _jp.apply_patch = apply_patch
    _installed = True


def uninstall():
    global _installed
    JsonPatch.from_diff = classmethod(_orig_from_diff)
    JsonPatchWithAppend.from_diff = classmethod(_orig_from_diff_append)
    JsonPatch.apply = _orig_apply
    _jp.make_patch = _orig_make_patch
    _jp.apply_patch = _orig_apply_patch
    _installed = False


_orig_make_patch = _jp.make_patch
_orig_apply_patch = _jp.apply_patch

# same documentation (and doctests) as the originals
_from_diff.__doc__ = _orig_from_diff.__doc__
make_patch.__doc__ = _orig_make_patch.__doc__
apply_patch.__doc__ = _orig_apply_patch.__doc__
_apply.__doc__ = _orig_apply.__doc__
