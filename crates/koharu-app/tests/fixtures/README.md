# Historical scene fixtures

These small synthetic files contain no user project data or image assets.
They are serialized with the historical koharu-core source, independently of
the current `session.rs::compat` decoders.

| Fixture | Historical source | Format |
| --- | --- | --- |
| scene-v1.bin | `9f25035f34357c4d1cd1a393db493a793b25996e` (parent of scene-v2 change `9602bc55`) | Headerless v1 |
| scene-v6.bin | `0b2d93e9` | KSCN + little-endian version 6 |

The paired JSON files are serialized from the same historical values and serve
as independent semantic expectations. The integration test reads those values
using current JSON defaults, then compares the entire migrated scene, including
node ordering, transforms, project font, image opacity/name, masks, text style,
font predictions and blob references. Fixture colours deliberately avoid old
auto-colour sentinels; the existing unit tests cover their conversion.

Regenerate from the repository root in PowerShell:

```powershell
& crates/koharu-app/tests/fixtures/generate.ps1
```

The script reads fixed local git objects into `target/compat-fixture-generator`,
builds with cached dependencies (`--offline`), and writes these fixture files.
It does not switch the checkout or use a current compat struct. `generate.rs`
specifies deterministic UUIDs, timestamps and synthetic content; its Snapshot
field order matches the historical `session.rs` writers.

SHA-256 of the generated binary fixtures:

- v1: `c1c9f01e8d5ffcfd70f210c111d4f8fbbfecbbc179bbe6e3d0b12b509a48bfa8`
- v6: `1e6343825f09ee5997b398e3e3da005128c9bc1aeb7ad056309b15320ea8e3aa`
