//go:build !mygo_noinspector

package main

// The automation plugin: started with MYGO_AUTOMATION=/path/to.sock, the
// app serves mygo-automation on that socket (snapshot, press, fill,
// screenshot). Release builds use -tags mygo_noinspector, which leaves it
// out.
import _ "github.com/egoist/mygo/plugins/automation"
