# Wastearr

A Python CLI tool that analyzes your Sonarr and Radarr collections to identify content with poor value-to-storage ratios. Connects directly to Sonarr/Radarr APIs to fetch ratings and file sizes, then calculates "waste scores" for movies and TV shows.

## Features

- **Direct API integration**: Connects to Sonarr and Radarr APIs for accurate metadata
- **Smart scoring**: Content-aware algorithm that accounts for different expectations between movies and TV shows  
- **Downloaded content only**: Shows only TV shows and movies with downloaded files by default
- **Intelligent caching**: Automatic caching with 72-hour expiration for faster subsequent runs
- **Flexible filtering**: Filter by waste score, file size, ratings, or show top offenders
- **Terminal-friendly**: Responsive table formatting that adapts to your terminal width
- **Robust error handling**: Clear error messages and connectivity validation

## Quick Start

```bash
# Set your API keys
export SONARR_API_KEY="your_sonarr_api_key"
export RADARR_API_KEY="your_radarr_api_key"

# Analyze both collections
./wastearr

# Analyze only TV series (Sonarr)
./wastearr sonarr

# Analyze only movies (Radarr)  
./wastearr radarr

# Show worst 20 items
./wastearr --top-waste 20

# Find large, low-rated content
./wastearr --min-size 5GB --ratings 6.0
```

## Requirements

- Python 3.8+
- [uv](https://docs.astral.sh/uv/) package manager
- Running Sonarr and/or Radarr instances
- API keys for Sonarr and Radarr

Dependencies are automatically managed via uv's inline script feature.

## Configuration

Wastearr supports multiple configuration methods with the following priority order:

1. **Environment variables** (highest priority)
2. **`.env` file** in current directory
3. **Config file** at `~/.config/wastearr/conf` (lowest priority)

### Configuration Options

**Required:**
- `SONARR_API_KEY` - Your Sonarr API key
- `RADARR_API_KEY` - Your Radarr API key

**Optional:**
- `SONARR_URL` - Sonarr URL (default: `http://localhost:8989`)
- `RADARR_URL` - Radarr URL (default: `http://localhost:7878`)

### Method 1: Environment Variables

```bash
export SONARR_API_KEY="your_sonarr_api_key"
export RADARR_API_KEY="your_radarr_api_key"
./wastearr
```

### Method 2: .env File

Create a `.env` file in the same directory as the script:

```bash
# Copy .env.sample to .env and edit
cp .env.sample .env
# Edit .env with your API keys
```

### Method 3: Config File

Create a config file at `~/.config/wastearr/conf`:

```bash
mkdir -p ~/.config/wastearr
cat > ~/.config/wastearr/conf << EOF
SONARR_API_KEY=your_sonarr_api_key
RADARR_API_KEY=your_radarr_api_key
SONARR_URL=http://localhost:8989
RADARR_URL=http://localhost:7878
EOF
```

### Getting API Keys

1. **Sonarr**: Settings → General → Security → API Key
2. **Radarr**: Settings → General → Security → API Key

## Options

- `sonarr` - Analyze TV series from Sonarr only
- `radarr` - Analyze movies from Radarr only
- `--top-waste N` - Show N highest waste scores
- `--waste-score N` - Show items with score ≥ N
- `--min-size SIZE` - Show items ≥ SIZE (e.g., 5GB, 500MB)
- `--ratings N` - Show items with rating ≤ N
- `--clear-cache` - Clear rating cache
- `--no-cache` - Bypass cache entirely

## How It Works

1. **API Connection**: Validates connectivity to Sonarr/Radarr APIs
2. **Data Retrieval**: Fetches series/movie metadata including ratings and file sizes
3. **Content Filtering**: Shows only content with downloaded files (sizeOnDisk > 0) by default
4. **Waste Score Calculation**: Combines file size and rating using content-aware algorithms
5. **Intelligent Display**: Shows results with responsive formatting and filtering options

## License

MIT