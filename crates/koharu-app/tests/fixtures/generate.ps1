# Regenerate synthetic golden snapshots using the actual historical Rust types.
# Run from the repository root. Uses only local git objects and cached packages.
$ErrorActionPreference = 'Stop'
$taskRepo = (Get-Location).Path
$taskFixtures = Join-Path $taskRepo 'crates/koharu-app/tests/fixtures'
$taskBuildRoot = Join-Path $taskRepo 'target/compat-fixture-generator'
$taskVersions = @(
    @{ Name = 'v1'; Revision = '9f25035f34357c4d1cd1a393db493a793b25996e'; Prefix = 'koharu-core/src'; Version = '1' },
    @{ Name = 'v6'; Revision = '0b2d93e9'; Prefix = 'crates/koharu-core/src'; Version = '6' }
)
$taskManifest = @'
[package]
name = "compat-fixture-generator"
version = "0.0.0"
edition = "2024"
[workspace]
[dependencies]
anyhow = "1.0"
chrono = { version = "0.4", features = ["serde", "clock"] }
indexmap = { version = "2.13", features = ["serde"] }
postcard = { version = "1.0", features = ["use-std"] }
schemars = { version = "1.2", features = ["chrono04", "indexmap2", "uuid1"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1.0"
strum = { version = "0.28", features = ["derive"] }
thiserror = "2.0"
utoipa = { version = "5", features = ["chrono", "indexmap", "uuid"] }
uuid = { version = "1.18", features = ["v4", "v7", "serde"] }
'@
foreach ($taskVersion in $taskVersions) {
    $taskWork = Join-Path $taskBuildRoot $taskVersion.Name
    $taskSrc = Join-Path $taskWork 'src'
    New-Item -ItemType Directory -Path $taskSrc -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $taskWork 'Cargo.toml') -Value $taskManifest -Encoding utf8
    foreach ($taskModule in @('scene', 'style', 'font', 'blob', 'op')) {
        $taskSpec = $taskVersion.Revision + ':' + $taskVersion.Prefix + '/' + $taskModule + '.rs'
        $taskSource = git show $taskSpec
        if ($LASTEXITCODE -ne 0) { throw "git show failed: $taskSpec" }
        Set-Content -LiteralPath (Join-Path $taskSrc ($taskModule + '.rs')) -Value $taskSource -Encoding utf8
    }
    Copy-Item -LiteralPath (Join-Path $taskFixtures 'generate.rs') -Destination (Join-Path $taskSrc 'main.rs')
    bun run scripts/dev.ts cargo run --offline --quiet --manifest-path (Join-Path $taskWork 'Cargo.toml') --target-dir (Join-Path $taskBuildRoot 'build') -- $taskVersion.Version $taskFixtures
    if ($LASTEXITCODE -ne 0) { throw "fixture generation failed: $($taskVersion.Name)" }
}
