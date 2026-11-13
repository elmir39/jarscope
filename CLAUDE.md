# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

- The plan, milestones and command specs are in `docs/project.md` (local only, gitignored). Build one milestone at a time.
- Java version from class major: 45 = 1.0/1.1, 46–48 = 1.2–1.4, ≥ 49 = major − 44. Minor `0xFFFF` = preview.
- `META-INF/versions/N/` counts only if the manifest has `Multi-Release: true`. `module-info.class` is reported separately. Neither counts toward the base Java requirement.
- Speed matters: `version` reads only 8 bytes per class, `find` reads only the ZIP's central directory.
- Never panic on bad JARs; in folder scans, warn and skip the bad JAR.
- Check output against `javap -v`, `jar tf` and `unzip -v`.
- Comments only for very important, non-obvious *why* (JVM quirks, safety limits); no doc comments that restate the code.
- `find`/`list`/`info` read via `cdir`, which yields every record like `jar tf`/`unzip`. Only `version` uses `zip`, which collapses duplicate names (`has_duplicate_names`) and warns.
- `find` never reads the manifest: `META-INF/versions/N/` classes match regardless of `Multi-Release`.
- `PathBuf` ordering is component-wise; sort paths bytewise with `as_encoded_bytes()` to match `LC_ALL=C sort`.
- macOS malloc serves blocks of ≤ 256 bytes per thread and size class; in folder scans avoid new small per-JAR allocations (see `cdir::SCRATCH_CAPACITY`).
