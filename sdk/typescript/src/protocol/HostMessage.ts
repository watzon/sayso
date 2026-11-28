import type { Event } from "./Event.js";
import type { Reply } from "./Reply.js";

/**
 * Any message from the host: a reply or an event.
 */
export type HostMessage = Reply | Event;
