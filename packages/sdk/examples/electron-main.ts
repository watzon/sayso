import { app, BrowserWindow, ipcMain } from "electron";
import { resolve } from "node:path";
import { Engine, Speech, microphonePermission } from "@justsayso/sdk";

let speech: Speech | undefined;

function running(): Speech {
	if (!speech) throw new Error("the speech features are not ready");
	return speech;
}

await app.whenReady();

speech = await Speech.connect({
	mode: "auto",
	app: { name: app.getName() },
	capabilities: ["models.download", "transcribe.file", "session.live", "audio.mic"],
	engine: Engine.fromPackage(),
});

speech.on((event) => {
	for (const window of BrowserWindow.getAllWindows()) {
		if (!window.isDestroyed()) window.webContents.send("speech:event", event);
	}
});

ipcMain.handle("speech:transcribe", (_event, file: string) => running().transcribe({ path: resolve(file) }));

ipcMain.handle("speech:start", async () => {
	if (!(await microphonePermission.request())) return null;
	const { session } = await running().startSession({ sources: ["mic"] });
	return session;
});

ipcMain.handle("speech:stop", (_event, session: number) => running().stopSession(session));

app.on("will-quit", (event) => {
	if (!speech) return;
	event.preventDefault();
	const closing = speech;
	speech = undefined;
	void closing.close().finally(() => app.quit());
});
