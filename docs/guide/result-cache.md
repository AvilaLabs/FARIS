<!-- kind: how-to -->
# The result cache

FARIS keeps results that cost minutes to make, such as weight windows, piece volumes and finished sensitivity points, so it does not calculate them twice. A kept result is used only when it was made from exactly the inputs you now have. This page says how that is decided and how to look after the cache.

## How a key is made

Every cached result has a key. One function, `result_cache::cache_key`, builds every key. A static check in the tests fails if another place builds one.

A key is the SHA-256 of the kind of result and its named components in canonical form. The components are everything that can change the result: the model, the materials, the response, the nuclear data library hash, the helper scripts' hashes, the seed, and similar. FARIS adds two components to every key itself: the FARIS version and the CPU architecture. A new release therefore never reuses an older release's results.

The canonical form means equal inputs give equal keys. The order of keys, white space, and the spelling of a number (`1`, `1.0`, `1e0`) do not change a key. A time stamp, a working folder or a host name is not a component and cannot change one. A value given in centimetres and the same length given in metres can be turned into one base value before it enters a key (`faris_model::canonical::to_base_unit`).

Some components are paths. The OpenMC Python used for weight windows enters the key as its path, because FARIS cannot see which libraries are installed in it. Moving the interpreter costs a rebuild; it can never cause a wrong reuse by moving, but upgrading OpenMC in place at the same path is not seen. Say so in your notes if you do that, or clear the cache.

## What an entry holds

An entry is a folder named `<kind>-<key>`. It holds the result files and `entry.json`, which lists:

- every component of the key, readable;
- the date it was made, the FARIS version, and the seconds it took;
- the SHA-256 and size of every file.

`faris cache explain KEY` prints these. Give at least six characters of the key.

## Every read is checked

On every read FARIS recomputes the key from the components stored in `entry.json`, compares it with the folder name, and hashes every file. If anything does not match, or a file is missing, or an unlisted file is there, the entry is refused, the reason is reported, and the result is calculated again. A damaged entry is never used. In a transport run the reason appears in the progress; in the sensitivity command it is printed.

An entry that is whole but was made for a different key than the one asked for is a false hit. It is refused and counted apart from the hits.

## Counters

`faris cache stats` shows hits, misses, entries refused as damaged and false hits, then two rates: the hit rate (hits over all lookups) and the false-hit rate (false hits over everything that looked like a hit). A high hit rate says nothing about whether the hits were right; the false-hit rate must be 0.

## Looking after the cache

All commands take `--dir FOLDER`. The default is `faris-transport-cache` in the current folder, where `faris design transport` and `faris reactor run` keep their results.

| Command | What it does |
| --- | --- |
| `faris cache list` | One line for each entry: size, when made, when last used |
| `faris cache verify` | Rehash every entry; exit 1 if any is damaged |
| `faris cache explain KEY` | Every component of one key with its value |
| `faris cache stats` | The counters and rates |
| `faris cache gc --max-size 10GB` | Remove the least recently used entries until the cache fits |
| `faris cache pin KEY`, `unpin KEY` | Keep an entry through `gc` and the size limit |
| `faris cache clear --yes` | Remove every entry, and the folders older FARIS versions left |

The cache is also bounded when you use it: when a new entry takes the total above 16 GiB, the least recently used entries that are not pinned are removed, never the one just stored.

Folders written by FARIS versions before this layer have no `entry.json`. They are never used. `faris cache verify` lists them as legacy and `faris cache clear --yes` removes them.

## Caches that do not use this layer

Two caches stay outside it, for stated reasons.

- The in-memory ensemble cache of the desktop holds typed keys that are compared field by field, nothing is hashed, and it never reaches disk.
- The ACTINV points cache (`faris maintenance`) is shared with a Python driver that names its files by their own hash. Changing the names would break that driver.

The tessellation and boolean caches of the CAD helper are kept by the helper beside the design, with a recorded key that is compared field by field.

## Resume a killed run

`faris history sensitivity ... --cache-dir FOLDER` keeps each finished point as soon as it is done. If the run is killed, run the same command again with the same folder: the finished points are reused and only the unfinished ones are calculated. The result file is byte for byte the one an uninterrupted run writes, because a point is kept only when reading it back gives the same value. At most the point in flight is lost. Each reused point is reported as "reused from <date>".

Resume covers this sweep. A killed OpenMC transport run still has to be run again from the start of that run; its finished recorded runs are already separate files and are not repeated. A killed Core evidence run is not covered.

Next: [Compare two studies or two models](compare-files.md).
