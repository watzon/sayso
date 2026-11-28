import type { Event } from "./Event.js";
import type { Reply } from "./Reply.js";

/**
 * Any message from the engine: a reply or an event.
 */
export type FromEngine = Reply | Event;
