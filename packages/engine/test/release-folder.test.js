import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { isAbsolute } from "node:path";
import { test } from "node:test";

import { platformPackage, releaseFolder, version } from "../index.js";

const require = createRequire(import.meta.url);

// The platforms that the engine supports, as Node names them.
const EXPECTED = {
	"darwin-arm64": "@justsayso/engine-darwin-arm64",
	"darwin-x64": "@justsayso/engine-darwin-x64",
	"linux-x64": "@justsayso/engine-linux-x64-gnu",
	"linux-arm64": "@justsayso/engine-linux-arm64-gnu",
	"win32-x64": "@justsayso/engine-win32-x64-msvc",
};

test("the version is the version of package.json", () => {
	assert.equal(version, require("../package.json").version);
});

test("the optional dependencies are the five platform packages, at this version", () => {
	const { optionalDependencies } = require("../package.json");
	assert.deepEqual(Object.keys(optionalDependencies).sort(), Object.values(EXPECTED).sort());
	for (const range of Object.values(optionalDependencies)) assert.equal(range, version);
});

test("platformPackage gives the package of this machine", () => {
	const name = platformPackage();
	if (process.platform === "linux") {
		// The libc decides: glibc has a package, musl has none.
		assert.ok(name === null || name === EXPECTED[`linux-${process.arch}`], String(name));
		return;
	}
	assert.equal(name, EXPECTED[`${process.platform}-${process.arch}`] ?? null);
});

test("platformPackage knows every supported platform", () => {
	for (const [key, name] of Object.entries(EXPECTED)) {
		const [platform, arch] = key.split("-");
		assert.equal(platformPackage({ platform, arch, libc: "glibc" }), name);
	}
});

test("platformPackage gives null for a platform without an engine", () => {
	assert.equal(platformPackage({ platform: "linux", arch: "x64", libc: "musl" }), null);
	assert.equal(platformPackage({ platform: "linux", arch: "arm", libc: "glibc" }), null);
	assert.equal(platformPackage({ platform: "win32", arch: "arm64" }), null);
	assert.equal(platformPackage({ platform: "freebsd", arch: "x64" }), null);
});

test("releaseFolder names the platform when the engine does not support it", () => {
	assert.throws(
		() => releaseFolder({ platform: "linux", arch: "x64", libc: "musl" }),
		(error) =>
			error instanceof Error &&
			error.message.includes("linux x64 with musl") &&
			error.message.includes("macOS (arm64 and x64)") &&
			error.message.includes("Linux (x64 and arm64, with glibc)") &&
			error.message.includes("Windows (x64)"),
	);
});

test("releaseFolder says what to do when the engine package is not installed", (t) => {
	const name = platformPackage();
	if (name === null) {
		t.skip("this machine has no engine package");
		return;
	}
	let installed = true;
	try {
		require.resolve(`${name}/package.json`);
	} catch {
		installed = false;
	}
	if (installed) {
		// Someone installed the engine package here. Then the folder must be real.
		const folder = releaseFolder();
		assert.ok(isAbsolute(folder));
		require(`${name}/package.json`);
		return;
	}
	assert.throws(
		() => releaseFolder(),
		(error) =>
			error instanceof Error &&
			error.message.includes(name) &&
			error.message.includes("is not installed") &&
			error.message.includes("--no-optional") &&
			error.message.includes("--omit=optional"),
	);
});
