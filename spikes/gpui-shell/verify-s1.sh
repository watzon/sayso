#!/bin/zsh
# usage: run_s1.sh <outprefix> [flags...]   (retries until the host app does not steal focus)
out=$1; shift
front() { osascript -e 'tell application "System Events" to get bundle identifier of first process whose frontmost is true'; }
for attempt in 1 2 3 4 5 6 7 8; do
  open -a TextEdit
  osascript -e 'tell application "TextEdit" to if (count of documents) = 0 then make new document' -e 'tell application "TextEdit" to set bounds of window 1 to {2200, 1100, 2950, 1440}' >/dev/null 2>&1
  for i in {1..20}; do [ "$(front)" = com.apple.TextEdit ] && break; sleep 0.5; done
  sleep 1
  echo "attempt $attempt pre-launch frontmost: $(front)" > /tmp/$out.ext
  /Users/watzon/Projects/personal/sayso/spikes/gpui-shell/target/debug/gpui-shell --overlay "$@" > /tmp/$out.log 2>&1 &
  PID=$!
  bad=0
  for t in {1..36}; do sleep 0.25; f=$(front); echo "ext t=$((t*25))cs frontmost=$f" >> /tmp/$out.ext; [ "$f" != com.apple.TextEdit ] && bad=1; [ $t = 20 ] && screencapture -x /tmp/$out.png; done
  kill $PID; wait $PID 2>/dev/null
  if [ $bad = 0 ]; then echo "VALID RUN (TextEdit frontmost for whole run, external sampler)" >> /tmp/$out.ext; break; fi
  echo "host stole focus; retry" >> /tmp/$out.ext
done
echo done > /tmp/$out.done
