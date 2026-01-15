# slashcmd

Natural language to shell commands.

```bash
$ slashcmd find files larger than 100mb
find . -type f -size +100M

$ slashcmd top 5 largest directories explain
du -sh */ | sort -hr | head -5

# du -sh */    → get size of each directory (human-readable)
# sort -hr     → sort by size, largest first
# head -5      → show only top 5

$ slashcmd delete all node_modules
find . -name "node_modules" -type d -exec rm -rf {} +

# [DANGER] Finds all folders named "node_modules" and deletes them permanently.
# ⚠ DANGER: Press Enter to copy to clipboard, Ctrl+C to cancel...
```

## Install

**Homebrew:**
```bash
brew install lgandecki/tap/slashcmd
```

**Script:**
```bash
curl -sSL slashcmd.lgandecki.net/install.sh | sh
```

**From source:**
```bash
git clone https://github.com/lgandecki/slashcmd
cd slashcmd/cli
cargo build --release
# Binary at target/release/slashcmd
```

## Usage

```bash
slashcmd login                     # Authenticate with GitHub
slashcmd find large files          # Get the command
slashcmd list all ports explain    # With human-readable explanation
slashcmd status                    # Check usage
```

**Shell alias** (add to `~/.zshrc`):
```bash
/cmd() { slashcmd "$@"; }
```

## Pricing

- **Free**: 100 commands (lifetime)
- **Pro**: $5/month unlimited

## Structure

```
cli/      # Rust CLI
worker/   # Cloudflare Worker API
site/     # Landing page
```

## License

MIT
