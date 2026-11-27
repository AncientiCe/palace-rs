#!/usr/bin/env node
// Starts the palace binary the user installed (on PATH) with the arguments
// given, passing stdin, stdout and stderr straight through. The plugin's MCP
// server and hooks run through this file so Claude Code starts them with node
// on every OS, with no shell in between.
"use strict";

const { spawn } = require("child_process");

const args = process.argv.slice(2);
const isHook = args[0] === "hook";

const child = spawn("palace", args, { stdio: "inherit", windowsHide: true });

child.on("error", (err) => {
  if (err.code === "ENOENT") {
    process.stderr.write(
      "palace: binary not found on PATH. Install it from " +
        "https://github.com/AncientiCe/palace-rs#-install, then restart Claude Code.\n"
    );
  } else {
    process.stderr.write(`palace: failed to start: ${err.message}\n`);
  }
  // A missing binary must not block the session through its hooks.
  process.exit(isHook ? 0 : 1);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => child.kill(signal));
}

child.on("exit", (code, signal) => {
  process.exit(code === null ? (signal ? 1 : 0) : code);
});
