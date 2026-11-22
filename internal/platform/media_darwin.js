// The now-playing reader and sender of internal/platform/media_darwin.go.
// It runs under /usr/bin/osascript -l JavaScript: since macOS 15.4 the
// private MediaRemote framework answers only Apple processes, and osascript
// is one.
//
//   get         prints the now-playing app as JSON, "null" when there is none,
//               or {"unsupported":true} when MRNowPlayingRequest is missing
//   send <cmd>  sends an MRMediaRemoteCommand (0 play, 1 pause)
ObjC.import('Foundation');

function run(argv) {
  $.NSBundle.bundleWithPath('/System/Library/PrivateFrameworks/MediaRemote.framework/').load;
  if (argv[0] === 'get') return get();
  if (argv[0] === 'send') return send(parseInt(argv[1], 10));
  throw new Error('usage: get | send <command>');
}

function isNil(v) {
  return v === undefined || v === null || (typeof v.isNil === 'function' && v.isNil());
}

function get() {
  const request = $.NSClassFromString('MRNowPlayingRequest');
  if (isNil(request)) return JSON.stringify({ unsupported: true });
  const path = request.localNowPlayingPlayerPath;
  const client = isNil(path) ? null : path.client;
  if (isNil(client)) return 'null';
  const out = { playing: !!request.localIsPlaying };
  const bundle = ObjC.unwrap(client.bundleIdentifier);
  if (bundle) out.bundleIdentifier = bundle;
  try {
    const parent = ObjC.unwrap(client.parentApplicationBundleIdentifier);
    if (parent) out.parentApplicationBundleIdentifier = parent;
  } catch (e) {}
  const pid = client.processIdentifier;
  if (pid > 0) out.processIdentifier = pid;
  return JSON.stringify(out);
}

function send(command) {
  if (isNaN(command)) throw new Error('send: the command is not a number');
  ObjC.bindFunction('MRMediaRemoteSendCommand', ['bool', ['int', 'id']]);
  const sent = $.MRMediaRemoteSendCommand(command, $());
  // MediaRemote sends the command from the run loop: run it for a moment so
  // the command leaves before osascript exits.
  $.NSRunLoop.currentRunLoop.runUntilDate($.NSDate.dateWithTimeIntervalSinceNow(0.3));
  return JSON.stringify({ sent: !!sent });
}
