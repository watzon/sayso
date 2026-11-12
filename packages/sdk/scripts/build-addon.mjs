// Build crates/sayso-node and put the addon next to the package as sayso.<platform>.node,
// where the loader finds it before the platform package. For the development of the SDK.
// Usage: node scripts/build-addon.mjs [--release] [--target <rust target>]

import { execFileSync } from "node:child_process";
import { copyFileSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const release = process.argv.includes("--release");
const targetIndex = process.argv.indexOf("--target");
const target = targetIndex > 0 ? process.argv[targetIndex + 1] : undefined;

const names = {
	"aarch64-apple-darwin": "darwin-arm64",
	"x86_64-apple-darwin": "darwin-x64",
	"x86_64-unknown-linux-gnu": "linux-x64-gnu",
	"aarch64-unknown-linux-gnu": "linux-arm64-gnu",
	"x86_64-pc-windows-msvc": "win32-x64-msvc",
};
const hostName = () => {
	if (process.platform === "linux") return `linux-${process.arch}-gnu`;
	if (process.platform === "win32") return `win32-${process.arch}-msvc`;
	return `${process.platform}-${process.arch}`;
};
const name = target ? names[target] : hostName();
if (!name) throw new Error(`no platform name for the target ${target}`);

const args = ["build", "-p", "sayso-node", "--locked"];
if (release) args.push("--release");
if (target) args.push("--target", target);
execFileSync("cargo", args, { cwd: root, stdio: "inherit" });

const file = { darwin: "libsayso_node.dylib", linux: "libsayso_node.so", win32: "sayso_node.dll" }[name.split("-")[0]];
const out = join(root, "target", ...(target ? [target] : []), release ? "release" : "debug", file);
const dest = join(root, "packages", "sdk", `sayso.${name}.node`);
// A new file, not a write over the old one: macOS kills a process that loads a signed
// file that changed in place.
rmSync(dest, { force: true });
copyFileSync(out, dest);
console.log(`${dest}`);
