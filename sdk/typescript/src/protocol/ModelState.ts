
/**
 * The state of a model.
 */
export type ModelState = "not_downloaded" | "downloading" | "downloaded" | "optimizing" | "ready" | "failed" | (string & {});
