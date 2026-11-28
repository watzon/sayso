import type { CatalogModel } from "./CatalogModel.js";
import type { Retired } from "./Retired.js";

/**
 * `ok` to `list_models`. Both lists are empty when the request had `models`.
 */
export type ListModelsReply = { models: Array<CatalogModel>, retired: Array<Retired>, };
