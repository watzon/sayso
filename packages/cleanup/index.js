// @justsayso/cleanup: finds the cleanup runtime package of this platform.
//
// The runtime `sayso-cleanup` runs the local cleanup models. It is in one of five
// packages, @justsayso/cleanup-<platform>. They are optional dependencies of this
// package, so a package manager installs only the one that fits the operating
// system, the CPU, and (on Linux) the libc.

import { createRequire } from "node:module";
import { dirname } from "node:path";

const require = createRequire(import.meta.url);

/** The version of this package. The runtime packages have the same version. */
export const version = require("./package.json").version;

const SCOPE = "@justsayso";

// `${platform}-${arch}` of Node, and the name of the runtime package.
const PACKAGES = {
	"darwin-arm64": `${SCOPE}/cleanup-darwin-arm64`,
	"darwin-x64": `${SCOPE}/cleanup-darwin-x64`,
	"linux-x64": `${SCOPE}/cleanup-linux-x64-gnu`,
	"linux-arm64": `${SCOPE}/cleanup-linux-arm64-gnu`,
	"win32-x64": `${SCOPE}/cleanup-win32-x64-msvc`,
};

const SUPPORTED = "macOS (arm64 and x64), Linux (x64 and arm64, with glibc), and Windows (x64)";

/**
 * "glibc" or "musl" for Linux, null for another system. When Node cannot say,
 * this assumes glibc: that is the only libc with a runtime package, and a
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
 * The name of the runtime package for this platform, or null when the cleanup
 * runtime has no package for it. The argument is for tests.
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
 * The absolute path of the folder of the runtime package of this platform.
 * The folder has the builds of the runtime `sayso-cleanup` and its files. On x64
 * for Linux and Windows it has one build for each CPU level. Give the folder to
 * `cleanup.bestRuntime()` of @justsayso/sdk to get the build that this processor
 * runs. The argument is for tests.
 *
 * @param {{ platform?: string, arch?: string, libc?: string }} [override]
 * @returns {string}
 */
export function runtimeFolder(override) {
	const name = platformPackage(override);
	if (name === null) {
		throw new Error(
			`@justsayso/cleanup has no cleanup runtime for ${describeTarget(target(override))}. ` +
				`The cleanup runtime supports ${SUPPORTED}.`,
		);
	}
	try {
		return dirname(require.resolve(`${name}/package.json`));
	} catch (error) {
		if (error?.code !== "MODULE_NOT_FOUND") throw error;
		throw new Error(
			`The cleanup runtime package ${name} is not installed. ` +
				"Install @justsayso/cleanup again without --no-optional or --omit=optional, " +
				"so that your package manager adds the runtime package of this platform.",
			{ cause: error },
		);
	}
}
