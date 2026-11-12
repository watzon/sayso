/** Choose another platform than the one that runs the code. For tests. */
export interface PlatformOverride {
	/** A value of `process.platform`, like `"darwin"`. */
	platform?: string;
	/** A value of `process.arch`, like `"arm64"`. */
	arch?: string;
	/** `"glibc"` or `"musl"`. Only Linux uses it. The default is `"glibc"`. */
	libc?: "glibc" | "musl";
}

/** The version of this package. The runtime packages have the same version. */
export const version: string;

/**
 * The name of the runtime package for this platform (like
 * `"@justsayso/cleanup-darwin-arm64"`), or `null` when the cleanup runtime has
 * no package for it.
 */
export function platformPackage(override?: PlatformOverride): string | null;

/**
 * The absolute path of the folder of the runtime package of this platform. The
 * folder has the builds of the runtime `sayso-cleanup`: one build, or on x64 for
 * Linux and Windows one for each CPU level.
 *
 * @throws Error when the cleanup runtime has no package for this platform, or
 * when the package is not installed.
 */
export function runtimeFolder(override?: PlatformOverride): string;
