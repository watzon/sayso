# How to run your app in a Flatpak

A connected app in a Flatpak reaches a running Sayso, wherever Sayso runs. Add one permission to your manifest. The SDK needs no other change.

## Add the permission

Add this to `finish-args` of your manifest:

```yaml
finish-args:
  - --filesystem=xdg-run/sayso:ro
```

It must be `:ro`. A sandbox with write access could replace the socket, and with `:create` Flatpak makes the folder with a mode that Sayso refuses. Then each app stays in local mode.

Without the permission, the SDK finds no socket, and your app runs in local mode. Your manifest must then have the engine ([How to ship the engine in your app](ship-the-engine.md)).

## Put your program in the app

Sayso knows your app by its Flatpak app id. The approval question shows the id, and `apps.toml` keeps it, so an update of your app needs no new approval.

The program that connects must be in `/app` or in the runtime. A program in the home folder of the sandbox gets `not_approved`, with no question.

Each program of your app has this identity. A program that your app starts must not use the socket of its parent: the first process that writes owns the connection, and a line from any other process closes it.

## Know what differs in a sandbox

- **Files.** Send a path that your sandbox sees. Sayso opens the file in the view of your sandbox, so `/app/share/talk.wav` works. A file that your sandbox cannot read gives `bad_request`, also when Sayso could read it.
- **`record_to`.** It gives `unsupported`, because Sayso and your app see other files. Start the session without it, and write the audio in your app.
- **Starting Sayso.** Your app cannot start Sayso from its sandbox. `Mode::Sayso { start: true }` gives an error. Use `Mode::Auto`, and the SDK changes to Sayso mode when Sayso starts.
- **The order of the start.** Flatpak can only show a folder that exists. If your app starts before the first start of Sayso since the login, it stays in local mode until your app starts again.
- **Two stores.** In local mode your app has the store of its sandbox, not of Sayso. A model that Sayso has can download a second time.

## Test it

```sh
flatpak info --show-permissions <your app id>   # filesystems has xdg-run/sayso:ro
flatpak run <your app id>
```

Start Sayso first. Sayso asks about your app, and the question shows `<your app id> · Flatpak`. After **Allow**, `Speech::mode()` gives Sayso mode.
