# Code walkthrough generator

Generates `ikigai-core-walkthrough.pdf` — a side-by-side reading of the kernel,
source on the left and annotations on the right.

The source is **sliced live from the committed crates** by `generate.py` (test
modules stripped), so the document can't drift from the code. Only the prose
lives in `walkthrough.toml`, as data: each `[[file]]` names a source file and
each `[[file.note]]` attaches bullets to the card whose source contains its
`anchor` (a unique substring).

## Build

```bash
pip install -r requirements.txt          # reportlab (rendering only)
python3 generate.py                       # → ikigai-core-walkthrough.pdf
python3 generate.py --out /some/where.pdf
```

Rendering embeds four TrueType fonts (base-14 PDF fonts render blank in some
viewers, so we embed). Defaults target macOS; override for other platforms:

```bash
python3 generate.py \
  --sans /path/Sans.ttf --sans-bold /path/Sans-Bold.ttf \
  --mono /path/Mono.ttf --mono-bold /path/Mono-Bold.ttf
# or set IKIGAI_WT_SANS / _SANS_BOLD / _MONO / _MONO_BOLD
```

## Check (CI)

```bash
python3 generate.py --check     # verify every anchor still resolves; no fonts needed
```

`--check` runs in CI (the *walkthrough anchors* job): if a refactor moves or
renames the code an annotation points at, the build fails until the manifest is
updated — the annotations stay honest.

## Adding / editing annotations

Edit `walkthrough.toml`. To annotate a new file, add a `[[file]]` block with its
`path`, `title`, `subtitle`, and one or more `[[file.note]]` entries. An `anchor`
must be a substring that appears **exactly once** in the file (after test modules
are stripped); `--check` enforces it.
