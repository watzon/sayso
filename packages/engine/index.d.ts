/** Choose another platform than the one that runs the code. For tests. */
export interface PlatformOverride {
	/** A value of `process.platform`, like `"darwin"`. */
	platform?: string;
	/** A value of `process.arch`, like `"arm64"`. */
	arch?: string;
	/** `"glibc"` or `"musl"`. Only Linux uses it. The default is `"glibc"`. */
	libc?: "glibc" | "musl";
}

/** The version of this package. The engine packages have the same version. */
export const version: string;

/**
 * The name of the engine package for this platform (like
 * `"@justsayso/engine-darwin-arm64"`), or `null` when the engine has no package
 * for it.
 */
export function platformPackage(override?: PlatformOverride): string | null;

/**
 * The absolute path of the folder of the engine package of this platform. The
 * folder has the engine program and its files.
 *
 * @throws Error when the engine has no package for this platform, or when the
 * package is not installed.
 */
export function releaseFolder(override?: PlatformOverride): string;
