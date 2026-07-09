const repoRoot = process.cwd();
const desktopDir = `${repoRoot}/apps/dou-voice-desktop`;
const skipFrontendInstall = Bun.argv.includes("--skip-frontend-install");

const homeDir = process.env.USERPROFILE || process.env.HOME;
const cargoBin = homeDir ? `${homeDir}/.cargo/bin` : "";
if (cargoBin) {
  process.env.PATH = `${cargoBin}${process.platform === "win32" ? ";" : ":"}${process.env.PATH}`;
}

function step(message) {
  console.log(`==> ${message}`);
}

function run(command, args, options = {}) {
  const result = Bun.spawnSync([command, ...args], {
    cwd: options.cwd || repoRoot,
    env: options.env || process.env,
    stdout: "inherit",
    stderr: "inherit",
    stdin: "ignore",
  });
  if (!result.success) {
    throw new Error(`${command} ${args.join(" ")} exited with ${result.exitCode}`);
  }
}

if (!skipFrontendInstall) {
  step("Installing frontend dependencies");
  run("bun", ["install"], { cwd: desktopDir });

  step("Building React frontend bundles");
  run("bun", ["run", "build:web"], { cwd: desktopDir });
}

const cargoEnv = {
  ...process.env,
  DOU_VOICE_SKIP_FRONTEND_BUILD: "1",
};

step("Checking Rust format");
run("cargo", ["fmt", "--all", "--", "--check"], { env: cargoEnv });

step("Checking workspace");
run("cargo", ["check", "--workspace"], { env: cargoEnv });

step("Running tests");
run("cargo", ["test", "--workspace"], { env: cargoEnv });

step("Running clippy");
run("cargo", ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"], {
  env: cargoEnv,
});

step("CI checks complete");
