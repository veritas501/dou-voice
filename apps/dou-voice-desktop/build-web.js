process.env.NODE_ENV = "production";

const onlyOverlay = Bun.argv.includes("--overlay");

const common = {
  target: "browser",
  format: "esm",
  minify: true,
  jsx: {
    runtime: "automatic",
    importSource: "react",
  },
  define: {
    "process.env.NODE_ENV": '"production"',
  },
};

const builds = [
  Bun.build({
    ...common,
    entrypoints: ["overlay-src/RecordingOverlay.jsx"],
    outdir: "web/scripts",
    naming: "overlay-react.js",
  }),
];

if (!onlyOverlay) {
  builds.push(
    Bun.build({
      ...common,
      entrypoints: ["main-src/App.jsx"],
      outdir: "web/scripts",
      naming: "main-react.js",
    }),
  );
}

const results = await Promise.all(builds);
for (const result of results) {
  if (!result.success) {
    for (const log of result.logs) {
      console.error(log);
    }
    process.exit(1);
  }
}
