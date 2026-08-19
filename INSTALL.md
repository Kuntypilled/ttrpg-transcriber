# Install

Linux, any distribution. Nothing here assumes a particular one.

## 1. Clone

```
git clone <your-repo-url> ttrpg-transcriber
cd ttrpg-transcriber
chmod +x transcribe.sh transcribe_gui.py launch_transcriber.sh
```

The scripts find each other from `__file__` and `BASH_SOURCE`, so the checkout can live
anywhere. There is no install step and nothing is copied into place.

## 2. System packages

Three things are not pip-installable.

`ffmpeg` and `jq` are spelled the same on every distribution.

**PyGObject / GTK3** is a system package. Installing it from pip is a well-known way to
lose an afternoon to build errors, because it compiles against system GObject headers.
Only `transcribe_gui.py` needs it. The three command-line stages run without it.

| Family | Command |
|---|---|
| Debian, Ubuntu, Zorin, Mint | `sudo apt install ffmpeg jq python3-gi python3-gi-cairo gir1.2-gtk-3.0` |
| Arch, CachyOS, EndeavourOS | `sudo pacman -S --needed ffmpeg jq python-gobject gtk3` |
| Fedora, RHEL | `sudo dnf install ffmpeg jq python3-gobject gtk3` |

The Debian row is the one this tool was developed against. The others follow each family's
usual naming. If a name is wrong on your system, find the right one rather than guessing:

```
pacman -Ss gobject          # Arch
apt search python3-gi       # Debian
dnf search gobject          # Fedora
```

Verify the bindings actually import, which is the only check that means anything:

```
python3 -c "import gi; gi.require_version('Gtk','3.0'); from gi.repository import Gtk; print('GTK3 ok')"
```

## 3. Python packages

```
python3 -m venv .venv && . .venv/bin/activate
pip install -r requirements.txt
```

A venv is the clean option but it complicates the GTK side, because PyGObject lives in
system Python and a plain venv cannot see it. Two ways out: create the venv with
`--system-site-packages`, or skip the venv and install for your user:

```
pip install --user --break-system-packages -r requirements.txt
```

The `--break-system-packages` flag is required on distributions that mark system Python as
externally managed (PEP 668). It is less alarming than it sounds: it installs into your own
`~/.local`, not into the system tree.

## 4. whisper.cpp

`whisper-cli` has to be on `$PATH`. The Vulkan backend is what this was developed against.
CUDA, ROCm, Metal and plain CPU all work identically as far as this tool is concerned,
because it only ever shells out to the binary.

```
git clone https://github.com/ggml-org/whisper.cpp
cd whisper.cpp
cmake -B build -DGGML_VULKAN=1
cmake --build build -j$(nproc) --config Release
sudo install -Dm755 build/bin/whisper-cli /usr/local/bin/whisper-cli
```

Those flags are upstream's, from the Vulkan section of the whisper.cpp README. Swap
`-DGGML_VULKAN=1` for the flag matching your backend if Vulkan is not what you want.

On Arch and derivatives a `whisper.cpp-vulkan` package exists in the AUR and may save you
the build. Read the PKGBUILD before installing it, as with anything from the AUR.

## 5. The model

```
mkdir -p ~/.local/share/whisper.cpp/models
wget -c -O ~/.local/share/whisper.cpp/models/ggml-large-v3-turbo.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin
```

Or, from a whisper.cpp checkout, `./models/download-ggml-model.sh large-v3-turbo`.

`large-v3-turbo` is roughly 1.5 GiB. Override the path with `--model /some/other.bin` if
you keep models elsewhere.

## 6. Point it at your vault

The vault root has no default, deliberately. There is no correct guess for where someone
keeps their vault, and a wrong guess fails quietly: the campaign glob matches nothing, the
dropdown comes up empty, and nothing tells you why.

Either export it:

```
export GM_VAULT=~/path/to/your/vault      # add to ~/.bashrc or ~/.zshrc to persist
```

Or write a config file, which the `.desktop` launcher can also see (a GUI launched from a
desktop entry does not inherit your shell's environment, so the config file is the more
reliable of the two):

```
mkdir -p ~/.config/ttrpg-transcriber
cp config.example.json ~/.config/ttrpg-transcriber/config.json
$EDITOR ~/.config/ttrpg-transcriber/config.json
```

Resolution order for every root: environment variable, then the config file, then a
built-in default.

| Setting | Env var | Default |
|---|---|---|
| vault | `GM_VAULT`, `VAULT` | none; unset is an error |
| audio | `GM_AUDIO_ROOT` | `$XDG_DATA_HOME/ttrpg_audio` |
| vector store | `GM_CHROMA_ROOT` | `$XDG_DATA_HOME/ttrpg_memory` |
| whisper scratch | `GM_WORK_ROOT` | `$XDG_CACHE_HOME/ttrpg_transcribe` |
| campaigns folder | `GM_CAMPAIGNS_SUBDIR` | `0.2_campaigns` |
| transcripts folder | `GM_TRANSCRIPTS_SUBDIR` | `08_transcripts` |
| GUI log | `GM_TRANSCRIBER_LOG` | `$XDG_STATE_HOME/ttrpg-transcriber/transcribe_gui.log` |
| config file | `GM_TRANSCRIBER_CONFIG` | `$XDG_CONFIG_HOME/ttrpg-transcriber/config.json` |

## 7. Create a campaign

A campaign is any folder under the vault's campaigns directory containing a
`canon_aliases.json`. That file is the entire registry.

```
mkdir -p "$GM_VAULT/0.2_campaigns/my_campaign"
cp canon_aliases.example.json "$GM_VAULT/0.2_campaigns/my_campaign/canon_aliases.json"
$EDITOR "$GM_VAULT/0.2_campaigns/my_campaign/canon_aliases.json"
```

Read the `_comment` fields in that file before editing. The `aliases` versus `review_only`
split is the part that matters and the part that is easy to get wrong in the direction that
silently damages a transcript.

## 8. Check the install

```
python3 campaign.py doctor
```

Prints every resolved root, whether it exists, and which of the three sources it came from.
Then:

```
./transcribe.sh --check
```

Checks `whisper-cli`, `ffmpeg`, `jq`, the model and the alias file, and prints the install
command for whichever package manager it finds on this machine.

## 9. Desktop entry, optional

```
cp ttrpg-transcriber.desktop.example ~/.local/share/applications/ttrpg-transcriber.desktop
$EDITOR ~/.local/share/applications/ttrpg-transcriber.desktop   # fix the Exec path
update-desktop-database ~/.local/share/applications
```

`.desktop` files do not expand `~` or `$HOME`, so `Exec` needs a full absolute path.

## Moving an existing install

Nothing needs migrating except audio.

The vector store default resolves to `~/.local/share/ttrpg_memory` on any system where
`XDG_DATA_HOME` is unset, and the scratch default to `~/.cache/ttrpg_transcribe`, which is
where they already were. `chromadb.PersistentClient` opens a store from any path, so copying
the directory across is enough. No re-ingest and no re-embedding.

The audio default did change, from a fixed `~/Documents` path to `$XDG_DATA_HOME/ttrpg_audio`.
Set `GM_AUDIO_ROOT`, or `audio_root` in the config file, to wherever your recordings actually
live. `python3 campaign.py doctor` will show it as `missing` until you do.
