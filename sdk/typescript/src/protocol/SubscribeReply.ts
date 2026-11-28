
/**
 * `ok` to `subscribe`: the events that the host sends to this connection
 * from now on. A name that the host does not know is not in the list.
 */
export type SubscribeReply = { events: Array<string>, };
