Audio fixtures for the job tests of `sayso-host` (`src/jobs/audio.rs`).

Each file is 1 s: silence, a 440 Hz sine at half of full scale from 0.3 s to 0.7 s, and silence. They were made for Sayso on 2026-10-26 from a generated signal, so they have no third-party content, and they have the license of Sayso:

```sh
ffmpeg -f lavfi -i "aevalsrc=if(between(t\,0.3\,0.7)\,0.5*sin(2*PI*440*t)\,0)|if(between(t\,0.3\,0.7)\,0.5*sin(2*PI*440*t)\,0):s=44100:d=1" -c:a pcm_s16le src.wav
lame --quiet -b 32 --resample 44.1 src.wav tone.mp3                          # MP3, 44.1 kHz stereo
ffmpeg -i src.wav -ac 1 -ar 8000 -c:a flac tone.flac                          # FLAC, 8 kHz mono
ffmpeg -i src.wav -ac 1 -ar 22050 -c:a aac -b:a 24k tone.m4a                  # AAC in M4A, 22.05 kHz mono
ffmpeg -i src.wav -ac 1 -ar 22050 -c:a aac -b:a 24k -f adts tone.aac          # AAC (ADTS), 22.05 kHz mono
```
