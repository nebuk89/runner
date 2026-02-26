// runner-plugin-host: Out-of-process plugin executor for the GitHub Actions Runner.
//
// This binary is spawned by the Runner.Worker to execute plugins in a separate
// process. It maps `Runner.PluginHost/Program.cs`.
//
// The library module is intentionally empty – all logic lives in `main.rs`.
// This file exists so the crate can be referenced as a library in tests.
