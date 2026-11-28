/**
 * The models of the app, for a request without a model in Local mode. A field that is missing takes the first suitable
 * model on disk. In Sayso mode, a request without a model gets the choice of the user, not these.
 */
export interface ModelChoices {
	finalModel?: string;
	previewModel?: string;
	/** A model id, or `basic` for the basic VAD. */
	vadModel?: string;
	diarizationModel?: string;
	/** A code such as `en`. The default is `auto`. */
	language?: string;
}
