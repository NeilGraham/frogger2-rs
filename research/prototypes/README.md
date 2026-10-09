# Format prototypes (Python, throwaway)

Used during research (2026-10-09) to verify formats against retail data. Run with `python3 -I`.
They contain no game data. Superseded by the Rust `f2-formats` crate once it exists.

| Script | Usage | Verifies |
|---|---|---|
| `iso9660_extract.py` | `iso9660_extract.py <track01.bin> [outdir]` | Lists/extracts ISO9660 from raw MODE1/2352 sectors |
| `pe_summary.py` | `pe_summary.py <file.dll ...>` | PE sections, relocs, exports |
| `fla_tex_wus_check.py` | `fla_tex_wus_check.py <Data_files dir>` | FLA2 LZSS, texture banks, `.wus` banks |
| `objbank_walk.py` | `objbank_walk.py <objects/x.dll>` | `GetbankAddress` → descriptor array → objects/meshes |

`DATA1.CAB` was extracted with `unshield -O -d out x data1.cab` (file must be named lowercase).
