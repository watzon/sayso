import type { ListedModel } from "./ListedModel.js";

/**
 * Without `models`, the engine checks the models of its built-in catalog.
 */
export type ListModels = { models?: Array<ListedModel>, };
