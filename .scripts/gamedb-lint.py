#!/usr/bin/env python3
"""Validate a gamedb data set: schemas, ids, and the one-game-one-page rule.

Usage: .scripts/gamedb-lint.py [directory]   (default: gamedb)

Interim stand-in for the Rust lint. Rules enforced beyond the schemas:

  * every page's canonical id is one its own identifiers justify, or a
    well-formed minted gamedb-<uid>
  * an id is assigned once and frozen; a page that later learns a better
    identifier keeps the id it had (reported as a warning, not an error)
  * every identifier a page carries - canonical id, <store>-<codename>, Steam
    app id, umu id, executables, and anything absorbed through merged_from -
    resolves to exactly one page
  * absorbed ids stay reserved: never a live id elsewhere, never absorbed twice
  * variant_of names a page that exists, live or retired
  * a umu id only appears when the page says umu-database named it
  * an id is not repeated where a store codename already carries it
  * a page identifies something: a store entry or an executable
  * the file name is a lowercase slug of the title, optionally qualified

Exit status is 1 if anything failed, so CI can gate a merge on it.
"""
import json, re, sys, tomllib, pathlib

def resolve(root, s):
    if "$ref" in s:
        p = s["$ref"].lstrip("#/").split("/")
        n = root
        for k in p: n = n[k]
        return n
    return s

def check(root, s, v, path, errs):
    s = resolve(root, s)
    t = s.get("type")
    if t == "object" and not isinstance(v, dict): errs.append(f"{path}: not an object"); return
    if t == "array" and not isinstance(v, list): errs.append(f"{path}: not an array"); return
    if t == "string" and not isinstance(v, str): errs.append(f"{path}: not a string"); return
    if "enum" in s and v not in s["enum"]: errs.append(f"{path}: {v!r} not in {s['enum']}")
    if isinstance(v, str):
        if "pattern" in s and not re.search(s["pattern"], v): errs.append(f"{path}: {v!r} fails /{s['pattern']}/")
        if len(v) < s.get("minLength", 0): errs.append(f"{path}: too short")
    if isinstance(v, list):
        if len(v) < s.get("minItems", 0): errs.append(f"{path}: too few items")
        for i, it in enumerate(v): check(root, s.get("items", {}), it, f"{path}[{i}]", errs)
    if isinstance(v, dict):
        if len(v) < s.get("minProperties", 0): errs.append(f"{path}: too few properties")
        for r in s.get("required", []):
            if r not in v: errs.append(f"{path}: missing required {r!r}")
        props, ap = s.get("properties", {}), s.get("additionalProperties", True)
        pn = s.get("propertyNames")
        for k, val in v.items():
            if pn and k not in resolve(root, pn).get("enum", [k]): errs.append(f"{path}.{k}: store key not allowed")
            if k in props: check(root, props[k], val, f"{path}.{k}", errs)
            elif ap is False: errs.append(f"{path}.{k}: unknown field")
            elif isinstance(ap, dict): check(root, ap, val, f"{path}.{k}", errs)

base = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "gamedb")
schemas = base/"schema" if (base/"schema").is_dir() else pathlib.Path("gamedb/schema")
gs = json.loads((schemas/"game.schema.json").read_text())
hs = json.loads((schemas/"helpers.schema.json").read_text())
ss = json.loads((schemas/"stores.schema.json").read_text())
stores = tomllib.loads((base/"stores.toml").read_text())
errs = []
for f in sorted((base/"games").glob("*.toml")):
    check(gs, gs, tomllib.loads(f.read_text()), f.name, errs)
check(hs, hs, tomllib.loads((base/"helpers.toml").read_text()), "helpers.toml", errs)
check(ss, ss, stores, "stores.toml", errs)

# stores.toml is the one list of stores: the page schema's store keys must be
# exactly its ids, less the standalone one, and the id precedence names only those.
listed = {s["id"] for s in stores.get("store", []) if not s.get("standalone")}
keys = set(gs["properties"]["stores"]["propertyNames"]["enum"])
if listed != keys:
    errs.append(f"game.schema.json: its store keys must match stores.toml (missing {sorted(listed - keys)}, not in stores.toml {sorted(keys - listed)})")
for p in stores.get("id_precedence", []):
    if p not in listed:
        errs.append(f"stores.toml: id_precedence names {p!r}, which is not a store it lists")
# what the schema cannot say, and gamebus's build refuses
seen = set()
for s in stores.get("store", []):
    for spelling in [s.get("id")] + s.get("aliases", []):
        if spelling in seen:
            errs.append(f"stores.toml: {json.dumps(spelling)} names two stores")
        seen.add(spelling)
standalone = sum(1 for s in stores.get("store", []) if s.get("standalone"))
if standalone != 1:
    errs.append(f'stores.toml: {standalone} standalone entries; exactly one says "no store"')

# canonical id: derived where an authority names the game, minted only when none does.
# the prefix names the namespace and the tail is never parsed, so gog-1660194629
# is a perfectly good id and needs no special handling.
STORE_PRECEDENCE = stores.get("id_precedence", [])

def candidates(d):
    """Every id this page's own data could justify, best first.

    [ids] outranks a store entry naming the same authority: a steam or umu
    store entry's codename IS a steam appid or umu id, so it justifies one
    exactly like [ids] would, just one rung lower - the page recorded the
    fact through a store rather than through [ids], nothing more."""
    out, ids = [], d.get("ids", {})
    if "steam" in ids:
        out.append(f"steam-{ids['steam']}")
    if "umu" in ids:
        out.append(ids["umu"])
    for e in d.get("stores", {}).get("steam", []):
        out.append(f"steam-{e['codename']}")
    for e in d.get("stores", {}).get("umu", []):
        out.append(e["codename"])
    for store in STORE_PRECEDENCE:
        for e in d.get("stores", {}).get(store, []):
            out.append(f"{store}-{e['codename']}")
    return out

def derive(d):
    ids = d.get("ids", {})
    if "steam" in ids:
        return f"steam-{ids['steam']}"
    if "umu" in ids:
        return ids["umu"]
    for e in d.get("stores", {}).get("steam", []):
        return f"steam-{e['codename']}"
    for e in d.get("stores", {}).get("umu", []):
        return e["codename"]
    for store in STORE_PRECEDENCE:
        for e in d.get("stores", {}).get(store, []):
            return f"{store}-{e['codename']}"
    return None

keys, ids_seen, variants, retired, warns, claims = {}, {}, [], {}, [], {}
for f in sorted((base/"games").glob("*.toml")):
    d = tomllib.loads(f.read_text())
    slug, ids = f.stem, d.get("ids", {})
    if not re.fullmatch(r"[a-z0-9]+(-[a-z0-9]+)*", slug):
        errs.append(f"{f.name}: file name is not a lowercase slug")
    tslug = re.sub(r"[^a-z0-9]+", "-", d["title"].lower()).strip("-")
    if slug != tslug and not slug.startswith(tslug + "-"):
        errs.append(f"{f.name}: file name is neither the title slug {tslug!r} nor that slug plus a qualifier")

    want, got, cands = derive(d), d.get("gamedb"), candidates(d)
    if got:
        minted = re.fullmatch(r"gamedb-[a-z0-9]*[a-z][a-z0-9]*", got)
        if got.startswith("gamedb-") and not minted:
            errs.append(f"{f.name}: minted id {got!r} must be gamedb-<uid> and must contain a letter")
        elif got not in cands and not minted:
            errs.append(f"{f.name}: gamedb is {got!r}, which none of this page's identifiers justifies")
        elif got != want and want is not None:
            # frozen on assignment: a page that later learns a better identifier keeps its id
            warns.append(f"{f.name}: id {got!r} predates {want!r}; kept, since ids are never re-pointed")
    elif want is None:
        errs.append(f"{f.name}: nothing names this game, so it needs a minted gamedb id")

    key = got or want
    if key:
        ids_seen[key] = f.name
    claimed = dict.fromkeys(cands + ([key] if key else []) + d.get("merged_from", []))
    for e in d.get("exe", []):
        claimed[f"exe:{e.lower()}"] = None
    for entries in d.get("stores", {}).values():
        for e in entries:
            if e.get("exe"):
                claimed[f"exe:{e['exe'].rsplit('/', 1)[-1].lower()}"] = None
    for ident in claimed:
        owner = claims.get(ident)
        if owner and owner != f.name:
            a, b = sorted([owner, f.name])
            errs.append(f"{ident!r} is claimed by both {a} and {b} - the same game cannot live on two "
                        f"pages, so consolidate them into one")
        claims[ident] = f.name
    keys[key] = f.name

    if "umu" in ids and ids.get("source") != "umu-database":
        errs.append(f"{f.name}: umu id present without an umu-database source")
    for store, entries in d.get("stores", {}).items():
        for e in entries:
            if ids.get(store) is not None and str(ids[store]) == e["codename"]:
                errs.append(f"{f.name}: ids.{store} repeats the {store} codename")
    if not d.get("stores") and not d.get("exe"):
        errs.append(f"{f.name}: names neither a store entry nor an executable, so it identifies nothing")
    if "variant_of" in d:
        variants.append((f.name, d["variant_of"]))

# retired ids stay reserved: never a live id elsewhere, never reused
for f in sorted((base/"games").glob("*.toml")):
    d = tomllib.loads(f.read_text())
    for old_id in d.get("merged_from", []):
        owner = ids_seen.get(old_id)
        if owner and owner != f.name:
            errs.append(f"{f.name}: merged_from {old_id!r} is the live id of {owner}")
        if old_id in retired and retired[old_id] != f.name:
            errs.append(f"{f.name}: merged_from {old_id!r} was already absorbed by {retired[old_id]}")
        retired[old_id] = f.name

for name, target in variants:
    if target not in ids_seen and target not in retired:
        errs.append(f"{name}: variant_of {target!r} names no page here")

print("\n".join("warning: " + w for w in warns)) if warns else None
print("\n".join(errs) if errs else f"OK: {len(list((base/'games').glob('*.toml')))} pages + helpers valid")
sys.exit(1 if errs else 0)
