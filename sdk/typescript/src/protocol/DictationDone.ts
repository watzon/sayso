
/**
 * A dictation after its insert.
 */
export type DictationDone = { 
/**
 * The inserted text.
 */
text: string, 
/**
 * The text of the speech model, before replacements and the style.
 */
transcript: string, app?: string, site?: string, style?: string, 
/**
 * When the insert happened, in RFC 3339.
 */
time: string, };
