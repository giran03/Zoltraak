# Generated media fixture

`verified-av.mp4` contains one second of a blue 64×64 video and a generated
440 Hz tone. It is used for real container/track/sample checks, without network
access or third-party media.

Regenerate from the project root with:

```powershell
ffmpeg -hide_banner -loglevel error -f lavfi -i "color=c=blue:s=64x64:r=10:d=1" -f lavfi -i "sine=frequency=440:duration=1" -c:v libx264 -pix_fmt yuv420p -c:a aac -movflags +faststart -y engine/core/tests/fixtures/verified-av.mp4
```
