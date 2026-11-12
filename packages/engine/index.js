// @justsayso/engine: finds the engine package of this platform.
//
// The engine itself is in one of five packages, @justsayso/engine-<platform>.
// They are optional dependencies of this package, so a package manager installs
// only the one that fits the operating system, the CPU, and (on Linux) the libc.

import { createRequire } from "node:module";
import { dirname } from "node:path";

const require = createRequire(import.meta.url);

/** The version of this package. The engine packages have the same version. */
export const version = require("./package.json").version;

const SCOPE = "@justsayso";

// `${platform}-${arch}` of Node, and the name of the engine package.
const PACKAGES = {
	"darwin-arm64": `${SCOPE}/engine-darwin-arm64`,
	"darwin-x64": `${SCOPE}/engine-darwin-x64`,
	"linux-x64": `${SCOPE}/engine-linux-x64-gnu`,
	"linux-arm64": `${SCOPE}/engine-linux-arm64-gnu`,
	"win32-x64": `${SCOPE}/engine-win32-x64-msvc`,
};

const SUPPORTED = "macOS (arm64 and x64), Linux (x64 and arm64, with glibc), and Windows (x64)";

/**
 * "glibc" or "musl" for Linux, null for another system. When Node cannot say,
 * this assumes glibc: that is the only libc with an engine package, and a
 * wrong guess ends in the "not installed" error, which says what to do.
 */
function detectLibc() {
	if (process.platform !== "linux") return null;
	try {
		const report = process.report;
		if (!report || typeof report.getReport !== "function") return "glibc";
		// The report looks up network addresses by default. That can be slow.
		const before = report.excludeNetwork;
		report.excludeNetwork = true;
		try {
			return report.getReport().header?.glibcVersionRuntime ? "glibc" : "musl";
		} finally {
			report.excludeNetwork = before;
		}
	} catch {
		return "glibc";
	}
}

function target(override) {
	return {
		platform: override?.platform ?? process.platform,
		arch: override?.arch ?? process.arch,
		libc: override?.libc ?? (override?.platform ? "glibc" : detectLibc()),
	};
}

function describeTarget({ platform, arch, libc }) {
	return `${platform} ${arch}` + (platform === "linux" && libc ? ` with ${libc}` : "");
}

/**
 * The name of the engine package for this platform, or null when the engine
 * has no package for it. The argument is for tests.
 *
 * @param {{ platform?: string, arch?: string, libc?: string }} [override]
 * @returns {string | null}
 */
export function platformPackage(override) {
	const t = target(override);
	if (t.platform === "linux" && t.libc !== "glibc") return null;
	return PACKAGES[`${t.platform}-${t.arch}`] ?? null;
}

/**
 * The absolute path of the folder of the engine package of this platform.
 * The folder has the engine program and its files.
 * The argument is for tests.
 *
 * @param {{ platform?: string, arch?: string, libc?: string }} [override]
 * @returns {string}
 */
export function releaseFolder(override) {
	const name = platformPackage(override);
	if (name === null) {
		throw new Error(
			`@justsayso/engine has no engine for ${describeTarget(target(override))}. ` +
				`The engine supports ${SUPPORTED}.`,
		);
	}
	try {
		return dirname(require.resolve(`${name}/package.json`));
	} catch (error) {
		if (error?.code !== "MODULE_NOT_FOUND") throw error;
		throw new Error(
			`The engine package ${name} is not installed. ` +
				"Install @justsayso/engine again without --no-optional or --omit=optional, " +
				"so that your package manager adds the engine package of this platform.",
			{ cause: error },
		);
	}
}
