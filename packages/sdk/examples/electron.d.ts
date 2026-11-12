// The few types of Electron that electron-main.ts uses, so the examples need no Electron package.
declare module "electron" {
	interface Event {
		preventDefault(): void;
	}

	class BrowserWindow {
		static getAllWindows(): BrowserWindow[];
		readonly webContents: { send(channel: string, ...args: unknown[]): void };
		isDestroyed(): boolean;
	}

	const app: {
		getName(): string;
		whenReady(): Promise<void>;
		quit(): void;
		on(name: "will-quit", listener: (event: Event) => void): void;
	};

	const ipcMain: {
		handle(channel: string, listener: (event: unknown, ...args: any[]) => unknown): void;
	};
}
