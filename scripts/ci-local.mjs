import { spawnSync } from "node:child_process";
import process from "node:process";

const stages = [
  ["Check formatting", "npm", ["run", "format:check"]],
  [
    "Check Rust formatting",
    "cargo",
    ["fmt", "--manifest-path", "src-tauri/Cargo.toml", "--", "--check"],
  ],
  ["Check capability boundary", "npm", ["run", "check:capabilities"]],
  ["Check locale contract", "npm", ["run", "check:locales"]],
  ["Lint frontend", "npm", ["run", "lint"]],
  ["Typecheck frontend", "npm", ["run", "typecheck"]],
  ["Test frontend", "npm", ["test"]],
  ["Test release scripts", "npm", ["run", "test:release"]],
  [
    "Lint Rust",
    "cargo",
    [
      "clippy",
      "--manifest-path",
      "src-tauri/Cargo.toml",
      "--all-targets",
      "--all-features",
      "--",
      "-D",
      "warnings",
    ],
  ],
  [
    "Test Rust",
    "cargo",
    ["test", "--manifest-path", "src-tauri/Cargo.toml", "--all-targets"],
    {
      // Fixture repositories create local commits. CI must not inherit a
      // developer's global commit.gpgSign preference or need their GPG agent.
      GIT_CONFIG_COUNT: "1",
      GIT_CONFIG_KEY_0: "commit.gpgSign",
      GIT_CONFIG_VALUE_0: "false",
    },
  ],
  ["Build frontend", "npm", ["run", "build:web"]],
  [
    "Build Tauri app without bundling",
    "npm",
    [
      "run",
      "tauri",
      "build",
      "--",
      "--no-bundle",
      "--target",
      "aarch64-apple-darwin",
    ],
  ],
];

for (const [name, command, args, environment] of stages) {
  process.stdout.write(`\n==> ${name}\n`);
  const result = spawnSync(command, args, {
    stdio: "inherit",
    env: environment ? { ...process.env, ...environment } : process.env,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
