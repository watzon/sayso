// Command sayso is the Sayso desktop app: the hub, the overlay, the tray
// item and the dictation controller, on MyGo.
//
//	sayso                      start, or show the running app
//	sayso --toggle             start or stop a dictation in the running app
//	sayso --version            print the version
//
// The other commands (--cancel, --paste-last, --cycle-style, --incognito,
// --style=<id>, --push-to-talk-down, --push-to-talk-up,
// --push-to-talk-up-submit, --open) go to the running app the same way;
// --pseudo shows the pseudo-locale. SAYSO_SMOKE=1 renders every surface
// without a window, in temporary folders, and exits.
package main

import (
	_ "embed"
	"flag"
	"fmt"
	"io"
	"log"
	"os"
	"runtime"

	"github.com/egoist/mygo"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/paths"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/internal/providers"
	"github.com/watzon/sayso/internal/store"
	"github.com/watzon/sayso/internal/theme"
	"github.com/watzon/sayso/internal/ui"
	"github.com/watzon/sayso/internal/update"
)

// releaseNotes is a copy of the repository's release-notes.toml
// (go generate; a test keeps them equal).
//
//go:generate cp ../../release-notes.toml release-notes.toml
//go:embed release-notes.toml
var releaseNotes string

// options are the command line.
type options struct {
	version bool
	pseudo  bool
	command ui.Command
}

func parseArgs(args []string, stderr io.Writer) (options, error) {
	fs := flag.NewFlagSet("sayso", flag.ContinueOnError)
	fs.SetOutput(stderr)
	var o options
	fs.BoolVar(&o.version, "version", false, "print the version")
	fs.BoolVar(&o.pseudo, "pseudo", false, "show the pseudo-locale (en-XA)")
	var style string
	fs.StringVar(&style, "style", "", "dictate once in this style")
	for _, f := range []string{"toggle", "cancel", "paste-last", "cycle-style", "incognito", "push-to-talk-down",
		"push-to-talk-up", "push-to-talk-up-submit", "open"} {
		fs.Bool(f, false, "send "+f+" to the running app")
	}
	if err := fs.Parse(args); err != nil {
		return o, err
	}
	o.command = ui.ParseCommand(args)
	return o, nil
}

func main() {
	o, err := parseArgs(os.Args[1:], os.Stderr)
	if err != nil {
		os.Exit(2)
	}
	if o.version {
		fmt.Println("Sayso", update.Version)
		return
	}
	smoke := os.Getenv("SAYSO_SMOKE") == "1"
	if smoke {
		if err := isolate(); err != nil {
			log.Fatal(err)
		}
	}
	if err := run(o, smoke); err != nil {
		log.Fatal(err)
	}
}

// isolate points every Sayso folder at a new temporary folder, so the smoke
// run never reads or writes the user's data.
func isolate() error {
	dir, err := os.MkdirTemp("", "sayso-smoke-")
	if err != nil {
		return err
	}
	for _, v := range []string{"XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"} {
		if err := os.Setenv(v, dir); err != nil {
			return err
		}
	}
	return nil
}

func run(o options, smoke bool) error {
	p := paths.Resolve(paths.SystemEnv{})
	if err := p.CreateAll(); err != nil {
		return err
	}
	mygo.App.SetName("Sayso")
	mygo.App.SetVersion(update.Version)
	if !smoke && !mygo.App.RequestSingleInstanceLock() {
		return nil // the running app got the command line
	}
	loaded, err := config.Load(p.ConfigFile())
	if err != nil {
		log.Printf("config: %v (using defaults)", err)
		loaded = config.Unreadable(p.ConfigFile(), err)
	}
	cfg := loaded.Config
	db, err := store.Open(p.DatabaseFile(), p.AudioDir())
	if err != nil {
		log.Printf("history: %v", err)
	}
	plat, err := platform.New()
	if err != nil {
		log.Printf("platform: %v", err)
	}
	var secrets providers.SecretStore = providers.KeychainStore{}
	if smoke {
		secrets = providers.NewMemoryStore()
	}
	apps := bridge.NewAppsFile(p.AppsFile())
	a := ui.New(ui.Deps{
		Paths: p, ConfigPath: p.ConfigFile(), Config: cfg, Issues: cfg.Validate(), Store: db, Platform: plat,
		Secrets: secrets, Apps: apps, Version: update.Version, ReleaseNotes: releaseNotes,
	})
	if o.pseudo {
		i18n.SetLocale(i18n.Pseudo)
	}
	if smoke {
		if db != nil {
			// Windows cannot delete the smoke run's folder while sayso.db is open.
			defer db.Close()
		}
		return ui.Smoke(a, os.Stdout)
	}
	return runApp(a, p, plat, db, secrets, apps, o)
}

// runApp starts the services and the windows, and runs until Quit.
func runApp(a *ui.App, p paths.Paths, plat *platform.Services, db *store.Store, secrets providers.SecretStore, apps *bridge.AppsFile, o options) error {
	desk := ui.NewDesktop(a)
	cfg := a.Config()

	// The speech engine.
	var speech *ui.EngineAdapter
	var ctl *dictation.Controller
	if path, err := ui.FindEngine(derefOr(cfg.Advanced.EnginePath)); err != nil {
		log.Printf("engine: %v", err)
	} else {
		speech, err = ui.StartEngine(engine.Options{EnginePath: path, ModelsDir: p.ModelsDir(), CacheDir: p.CacheDir, StoreDir: p.ModelStoreDir()},
			func() string { return ui.PreloadModel(a.ControllerConfig()) },
			func(ev engine.Event) {
				switch ev.Kind {
				case engine.EventPartial:
					if ctl != nil {
						ctl.EnginePartial(ev.Session, ev.Committed, ev.Tentative)
					}
				case engine.EventModelStatus:
					if ctl != nil && ev.Status.State == engine.StateReady {
						ctl.ModelReady(models.ModelID(ev.Model))
					}
				}
				mygo.RunOnMain(desk.Invalidate)
			})
		if err != nil {
			log.Printf("engine: %v", err)
		} else {
			a.SetEngine(speech)
		}
	}

	// Connected apps.
	var host *bridge.Host
	if hc, err := bridge.HostConfigForPaths(p); err != nil {
		log.Printf("host: %v", err)
	} else {
		hc.Apps, hc.HostVersion = apps, update.Version
		if host, err = bridge.StartHost(hc, &ui.HostServices{App: a, Speech: speech}); err != nil {
			log.Printf("host: %v", err)
		} else {
			a.SetHost(host)
		}
	}

	// The dictation controller.
	deps := dictation.Deps{
		Secrets: secrets, Config: a.ControllerConfig, Styles: a.ControllerStyles, Replacer: a.ControllerReplacer,
		Vocabulary: a.ControllerVocabulary, Incognito: a.ControllerIncognito, AppVersion: update.Version, Observer: a,
		SoundData: func(cue platform.SoundCue) []byte { return theme.SoundWAV(string(cue)) },
		System:    systemInfo,
		StyleName: func(s *enhance.Style) string { return s.Name },
	}
	if plat != nil {
		deps.Platform = *plat
	}
	if speech != nil {
		deps.Engine = speech
	}
	if db != nil {
		deps.History = db
	}
	if host != nil {
		deps.Host = host
	}
	ctl = dictation.New(deps)
	a.SetDictation(ctl)
	a.SetStyler(&dictation.DefaultStyler{Secrets: secrets, Config: a.ControllerConfig})

	// Global hotkeys.
	if plat != nil && plat.Hotkeys != nil {
		if err := plat.Hotkeys.Configure(ui.HotkeyConfig(cfg)); err != nil {
			log.Printf("hotkeys: %v", err)
		}
		if err := plat.Hotkeys.Start(func(ev platform.HotkeyEvent) { mygo.RunOnMain(func() { a.HandleHotkey(ev) }) }); err != nil {
			log.Printf("hotkeys: %v", err)
		}
	}

	// Updates.
	var updater *update.Updater
	if uo, ok := update.OptionsForThisBuild(p.DataDir, p.CacheDir, cfg.Updates.Check, cfg.Updates.Automatic); ok {
		updater = update.Spawn(uo, a.UpdateStatus)
		a.SetUpdater(updater)
	}

	mygo.App.OnSecondInstance(func(args []string, _ string) {
		mygo.RunOnMain(func() {
			cmd := ui.ParseCommand(args)
			if cmd.Kind == ui.CmdNone {
				cmd.Kind = ui.CmdOpen
			}
			a.RunCommand(cmd)
		})
	})
	if runtime.GOOS == "darwin" && !cfg.General.DockIconWithHub {
		mygo.App.SetActivationPolicy(mygo.ActivationPolicyAccessory)
	}
	mygo.App.WhenReady(func() {
		show := o.command.Kind == ui.CmdNone || o.command.Kind == ui.CmdOpen
		desk.Start(show && !a.NeedsOnboarding())
		desk.Tick()
		if o.command.Kind != ui.CmdNone {
			a.RunCommand(o.command)
		}
		if !a.NeedsOnboarding() && a.HasWhatsNew() && update.WhatsNewDue(update.WhatsNewSeen(p.StateFile()), update.MustVersion(update.Version)) {
			a.OpenWhatsNew()
		}
	})
	mygo.App.OnWillQuit(func(*mygo.QuitEvent) {
		ctl.Close()
		enhance.StopSpares()
		if plat != nil && plat.Hotkeys != nil {
			plat.Hotkeys.Stop()
		}
		if host != nil {
			host.Close()
		}
		if speech != nil {
			speech.Close()
		}
		if updater != nil {
			updater.Stop()
		}
		if db != nil {
			db.Close()
		}
	})
	return mygo.App.Run()
}

func derefOr(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

func systemInfo() store.SystemInfo {
	return store.SystemInfo{OS: runtime.GOOS, Arch: runtime.GOARCH}
}
