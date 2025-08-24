#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.8"
# dependencies = [
#     "tabulate",
#     "requests",
#     "platformdirs",
# ]
# ///

import os
import re
import time
import json
import argparse
import shutil
from pathlib import Path
from tabulate import tabulate
import requests
from statistics import mode, median
from platformdirs import user_cache_dir, user_config_dir

CACHE_FILE = Path(user_cache_dir("wastearr")) / "cache.json"
CACHE_DURATION = 72 * 60 * 60  # 72 hours in seconds

def load_env_file(env_file_path):
    """Load environment variables from a .env file"""
    env_vars = {}
    if env_file_path.exists():
        try:
            with open(env_file_path, 'r') as f:
                for line in f:
                    line = line.strip()
                    if line and not line.startswith('#') and '=' in line:
                        key, value = line.split('=', 1)
                        key = key.strip()
                        value = value.strip().strip('"').strip("'")
                        env_vars[key] = value
        except (OSError, IOError):
            pass
    return env_vars

def load_config_file(config_file_path):
    """Load configuration from user config directory"""
    config_vars = {}
    if config_file_path.exists():
        try:
            with open(config_file_path, 'r') as f:
                for line in f:
                    line = line.strip()
                    if line and not line.startswith('#') and '=' in line:
                        key, value = line.split('=', 1)
                        key = key.strip()
                        value = value.strip().strip('"').strip("'")
                        config_vars[key] = value
        except (OSError, IOError):
            pass
    return config_vars

def get_config_value(key, default=None):
    """Get configuration value with priority: env vars > .env file > config file > default"""
    # 1. Check environment variables first
    value = os.getenv(key)
    if value is not None:
        return value
    
    # 2. Check .env file in current directory
    env_file = Path.cwd() / ".env"
    env_vars = load_env_file(env_file)
    if key in env_vars:
        return env_vars[key]
    
    # 3. Check user config directory
    config_dir = Path(user_config_dir("wastearr"))
    config_file = config_dir / "conf"
    config_vars = load_config_file(config_file)
    if key in config_vars:
        return config_vars[key]
    
    # 4. Return default value
    return default

# Sonarr/Radarr Configuration with multi-source loading
SONARR_URL = get_config_value("SONARR_URL", "http://localhost:8989")
SONARR_API_KEY = get_config_value("SONARR_API_KEY")
RADARR_URL = get_config_value("RADARR_URL", "http://localhost:7878")
RADARR_API_KEY = get_config_value("RADARR_API_KEY")

class Item:
    def __init__(self, name, year, size_bytes, rating, item_type, identifier=None):
        self.name = name
        self.year = year
        self.size_bytes = size_bytes
        self.rating = rating
        self.item_type = item_type  # 'tv' or 'movie'
        self.identifier = identifier
        self.waste_score = 0

def load_cache():
    """Load entire cache from file, return dict with sonarr and radarr caches"""
    if not CACHE_FILE.exists():
        print("No existing cache found")
        return {'sonarr': {}, 'radarr': {}}

    try:
        print(f"Loading cache from {CACHE_FILE}")
        with open(CACHE_FILE, 'r') as f:
            cache_data = json.load(f)

        cache_timestamp = cache_data.get('timestamp', 0)
        current_time = time.time()

        if current_time - cache_timestamp > CACHE_DURATION:
            print("Cache expired, removing old cache file")
            CACHE_FILE.unlink(missing_ok=True)
            return {'sonarr': {}, 'radarr': {}}

        # Support both old and new cache formats for migration
        sonarr_cache = cache_data.get('sonarr_ratings', cache_data.get('tv_ratings', {}))
        radarr_cache = cache_data.get('radarr_ratings', cache_data.get('movie_ratings', {}))

        return {
            'sonarr': sonarr_cache,
            'radarr': radarr_cache
        }

    except (json.JSONDecodeError, KeyError, OSError):
        print("Cache corrupted, starting fresh")
        CACHE_FILE.unlink(missing_ok=True)
        return {'sonarr': {}, 'radarr': {}}

def save_cache(sonarr_cache, radarr_cache):
    """Save cache to file with current timestamp"""
    try:
        cache_data = {
            'timestamp': time.time(),
            'sonarr_ratings': sonarr_cache,
            'radarr_ratings': radarr_cache
        }

        total_ratings = len(sonarr_cache) + len(radarr_cache)
        print(f"Saving cache with {total_ratings} ratings")

        CACHE_FILE.parent.mkdir(parents=True, exist_ok=True)
        with open(CACHE_FILE, 'w') as f:
            json.dump(cache_data, f)

    except OSError:
        pass

def get_sonarr_series():
    """Fetch all series from Sonarr API"""
    if not SONARR_API_KEY:
        print("Error: SONARR_API_KEY environment variable not set")
        return []

    try:
        url = f"{SONARR_URL}/api/v3/series"
        headers = {
            "X-Api-Key": SONARR_API_KEY,
            "Content-Type": "application/json"
        }

        response = requests.get(url, headers=headers, timeout=10)

        if response.status_code == 200:
            series_list = response.json()
            print(f"Fetched {len(series_list)} series from Sonarr API")
            return series_list
        else:
            print(f"Failed to fetch series from Sonarr API: HTTP {response.status_code}")
            return []

    except requests.exceptions.RequestException as e:
        print(f"Failed to connect to Sonarr API: {e}")
        return []
    except Exception as e:
        print(f"Error fetching series from Sonarr API: {e}")
        return []

def get_sonarr_rating(series_id, cache=None):
    """Get rating for a specific series from cache or return N/A"""
    # Skip cache lookup if cache is None (--no-cache mode)
    if cache is not None and str(series_id) in cache:
        return cache[str(series_id)]

    # This will be populated by the series data during scanning
    # For now, return N/A and let the scanning logic handle it
    return "N/A"

def get_radarr_movies():
    """Fetch all movies from Radarr API"""
    if not RADARR_API_KEY:
        print("Error: RADARR_API_KEY environment variable not set")
        return []

    try:
        url = f"{RADARR_URL}/api/v3/movie"
        headers = {
            "X-Api-Key": RADARR_API_KEY,
            "Content-Type": "application/json"
        }

        response = requests.get(url, headers=headers, timeout=10)

        if response.status_code == 200:
            movies_list = response.json()
            print(f"Fetched {len(movies_list)} movies from Radarr API")
            return movies_list
        else:
            print(f"Failed to fetch movies from Radarr API: HTTP {response.status_code}")
            return []

    except requests.exceptions.RequestException as e:
        print(f"Failed to connect to Radarr API: {e}")
        return []
    except Exception as e:
        print(f"Error fetching movies from Radarr API: {e}")
        return []

def get_radarr_rating(movie_id, cache=None):
    """Get rating for a specific movie from cache or return N/A"""
    # Skip cache lookup if cache is None (--no-cache mode)
    if cache is not None and str(movie_id) in cache:
        return cache[str(movie_id)]

    # This will be populated by the movie data during scanning
    # For now, return N/A and let the scanning logic handle it
    return "N/A"

def validate_api_connectivity(scan_types):
    """Validate that required APIs are accessible"""
    api_errors = []
    
    for scan_type in scan_types:
        if scan_type == 'sonarr':
            if not SONARR_API_KEY:
                api_errors.append("SONARR_API_KEY environment variable not set")
                continue
            
            try:
                url = f"{SONARR_URL}/api/v3/system/status"
                headers = {"X-Api-Key": SONARR_API_KEY}
                response = requests.get(url, headers=headers, timeout=5)
                if response.status_code != 200:
                    api_errors.append(f"Sonarr API unreachable at {SONARR_URL} (HTTP {response.status_code})")
            except requests.exceptions.RequestException as e:
                api_errors.append(f"Cannot connect to Sonarr at {SONARR_URL}: {e}")
        
        elif scan_type == 'radarr':
            if not RADARR_API_KEY:
                api_errors.append("RADARR_API_KEY environment variable not set")
                continue
                
            try:
                url = f"{RADARR_URL}/api/v3/system/status"
                headers = {"X-Api-Key": RADARR_API_KEY}
                response = requests.get(url, headers=headers, timeout=5)
                if response.status_code != 200:
                    api_errors.append(f"Radarr API unreachable at {RADARR_URL} (HTTP {response.status_code})")
            except requests.exceptions.RequestException as e:
                api_errors.append(f"Cannot connect to Radarr at {RADARR_URL}: {e}")
    
    return api_errors

def get_tmdb_rating_tv(show_name, year, cache=None):
    """Fetch TMDB rating for TV show using search API with caching"""
    cache_key = f"{show_name}_{year}"

    # Skip cache lookup if cache is None (--no-cache mode)
    if cache is not None and cache_key in cache:
        return cache[cache_key]

    bearer_token = os.getenv("TMDB_API_KEY")
    if not bearer_token:
        return "N/A"

    try:
        search_url = f"https://api.themoviedb.org/3/search/tv"
        headers = {
            "Authorization": f"Bearer {bearer_token}",
            "Content-Type": "application/json"
        }

        params = {
            "query": show_name,
            "first_air_date_year": year
        }

        response = requests.get(search_url, headers=headers, params=params, timeout=5)

        if response.status_code == 200:
            data = response.json()
            results = data.get('results', [])

            if results:
                show_data = results[0]
                actual_name = show_data.get('name', show_name)
                tmdb_id = show_data.get('id', 'Unknown')
                rating = show_data.get('vote_average', 'N/A')
                result = f"{rating:.1f}" if rating != 'N/A' and rating != 0 else "N/A"

                print(f"Fetched '{actual_name}' [TMDB:{tmdb_id}] from TMDB API")

                if cache is not None:
                    cache[cache_key] = result

                return result

        print(f"Failed to fetch '{show_name}' ({year}) from TMDB API")

        if cache is not None:
            cache[cache_key] = "N/A"

        return "N/A"

    except Exception:
        print(f"Failed to fetch '{show_name}' ({year}) from TMDB API")

        if cache is not None:
            cache[cache_key] = "N/A"

        return "N/A"

def get_tmdb_rating_movie(tmdb_id, cache=None):
    """Fetch TMDB rating for movie using direct API with caching"""
    if not tmdb_id:
        return "N/A"

    # Skip cache lookup if cache is None (--no-cache mode)
    if cache is not None and str(tmdb_id) in cache:
        return cache[str(tmdb_id)]

    bearer_token = os.getenv("TMDB_API_KEY")
    if not bearer_token:
        return "N/A"

    try:
        url = f"https://api.themoviedb.org/3/movie/{tmdb_id}"
        headers = {
            "Authorization": f"Bearer {bearer_token}",
            "Content-Type": "application/json"
        }

        response = requests.get(url, headers=headers, timeout=5)

        if response.status_code == 200:
            data = response.json()
            movie_title = data.get('title', 'Unknown')
            rating = data.get('vote_average', 'N/A')
            result = f"{rating:.1f}" if rating != 'N/A' and rating != 0 else "N/A"

            print(f"Fetched '{movie_title}' [TMDB:{tmdb_id}] from TMDB API")

            if cache is not None:
                cache[str(tmdb_id)] = result

            return result

        print(f"Failed to fetch movie [TMDB:{tmdb_id}] from TMDB API")

        if cache is not None:
            cache[str(tmdb_id)] = "N/A"

        return "N/A"

    except Exception:
        print(f"Failed to fetch movie [TMDB:{tmdb_id}] from TMDB API")

        if cache is not None:
            cache[str(tmdb_id)] = "N/A"

        return "N/A"

def extract_tv_info(show_dir_name, total_size):
    """Extract show name, year, and TVDB ID from directory name"""
    pattern = r'^(.+?)\s*\((\d{4})\)\s*(?:\[tvdbid-(\d+)\])?$'

    match = re.match(pattern, show_dir_name)
    if match:
        show_name = match.group(1).strip()
        year = match.group(2)
        tvdb_id = match.group(3) if match.group(3) else None
        return show_name, year, tvdb_id, total_size

    fallback_pattern = r'^(.+?)\s*\((\d{4})\).*$'
    fallback_match = re.match(fallback_pattern, show_dir_name)
    if fallback_match:
        show_name = fallback_match.group(1).strip()
        year = fallback_match.group(2)
        return show_name, year, None, total_size

    return None

def extract_movie_info(filename, file_size):
    """Extract movie name, year, and TMDB ID from filename"""
    pattern = r'^(.+?)\s*\((\d{4})\)\s*(?:\[tmdbid-(\d+)\])?\s*-?\s*.*\.(mkv|mp4|avi)$'

    match = re.match(pattern, filename)
    if match:
        movie_name = match.group(1).strip()
        year = match.group(2)
        tmdb_id = match.group(3) if match.group(3) else None
        return movie_name, year, tmdb_id, file_size

    fallback_pattern = r'^(.+?)\s*\((\d{4})\).*\.(mkv|mp4|avi)$'
    fallback_match = re.match(fallback_pattern, filename)
    if fallback_match:
        movie_name = fallback_match.group(1).strip()
        year = fallback_match.group(2)
        return movie_name, year, None, file_size

    return None

def format_file_size(size_bytes):
    """Convert bytes to human readable format"""
    for unit in ['B', 'KB', 'MB', 'GB', 'TB']:
        if size_bytes < 1024.0:
            return f"{size_bytes:.1f} {unit}"
        size_bytes /= 1024.0
    return f"{size_bytes:.1f} PB"

def parse_size_string(size_str):
    """Parse size string like '12M', '3GB', '500MB' to bytes"""
    import re

    # Match number followed by optional unit
    match = re.match(r'^(\d+(?:\.\d+)?)\s*([KMGTB]?B?)?$', size_str.upper())
    if not match:
        raise ValueError(f"Invalid size format: {size_str}")

    number = float(match.group(1))
    unit = match.group(2) or 'B'

    # Normalize unit (handle both 'M' and 'MB' formats)
    if unit == 'B':
        multiplier = 1
    elif unit in ['K', 'KB']:
        multiplier = 1024
    elif unit in ['M', 'MB']:
        multiplier = 1024 ** 2
    elif unit in ['G', 'GB']:
        multiplier = 1024 ** 3
    elif unit in ['T', 'TB']:
        multiplier = 1024 ** 4
    else:
        raise ValueError(f"Unknown unit: {unit}")

    return int(number * multiplier)

def calculate_size_score(size_bytes):
    """Calculate base size score 0-80 using logarithmic scaling"""
    size_gb = size_bytes / (1024 ** 3)

    import math
    if size_gb <= 1:
        size_score = size_gb * 10
    else:
        size_score = 10 + (math.log10(size_gb) * 30)
    return min(size_score, 80)

def get_tv_rating_multiplier(rating):
    """TV shows get more forgiving rating curve (longer content)"""
    if rating >= 8.0:
        return 0.05  # Excellent shows: 95% penalty reduction
    elif rating >= 7.5:
        return 0.15  # Very good shows: 85% penalty reduction
    elif rating >= 7.0:
        return 0.35  # Good shows: 65% penalty reduction
    elif rating >= 6.5:
        return 0.55  # Decent shows: 45% penalty reduction
    elif rating >= 6.0:
        return 0.75  # Average shows: 25% penalty reduction
    else:
        return 1.1   # Poor shows: 10% penalty increase

def get_movie_rating_multiplier(rating):
    """Movies get standard rating curve (stricter standards)"""
    if rating >= 8.0:
        return 0.1   # Excellent movies: 90% penalty reduction
    elif rating >= 7.5:
        return 0.2   # Very good movies: 80% penalty reduction
    elif rating >= 7.0:
        return 0.4   # Good movies: 60% penalty reduction
    elif rating >= 6.5:
        return 0.6   # Decent movies: 40% penalty reduction
    elif rating >= 6.0:
        return 0.8   # Average movies: 20% penalty reduction
    else:
        return 1.2   # Poor movies: 20% penalty increase

def calculate_normalized_waste_score(item):
    """Content-aware waste scoring with type-specific normalization"""
    if item.rating == "N/A":
        rating = 6.0
    else:
        rating = float(item.rating)

    # Base size score (same logarithmic scaling)
    base_size_score = calculate_size_score(item.size_bytes)

    # Content-type normalization
    if item.item_type == 'tv':
        # TV shows are expected to be larger (multi-season content)
        normalized_size = base_size_score * 0.6  # 40% size discount
        rating_multiplier = get_tv_rating_multiplier(rating)
    else:  # movies
        # Movies should be more size-efficient
        normalized_size = base_size_score * 1.0  # No size adjustment
        rating_multiplier = get_movie_rating_multiplier(rating)

    waste_score = normalized_size * rating_multiplier
    return int(round(max(0, min(100, waste_score))))

def sum_video_files(directory):
    """Calculate total size of all video files in a directory tree"""
    total_size = 0
    for root, dirs, files in os.walk(directory):
        for file in files:
            file_path = Path(root) / file
            try:
                if file_path.suffix.lower() in ['.mkv', '.mp4', '.avi', '.m4v']:
                    total_size += file_path.stat().st_size
            except (OSError, PermissionError):
                continue
    return total_size

def is_video_file(file_path):
    """Check if file is a video file"""
    return file_path.suffix.lower() in ['.mkv', '.mp4', '.avi', '.m4v']

def truncate_text(text, max_length):
    """Truncate text to max_length, adding ellipsis if needed"""
    if len(text) <= max_length:
        return text
    return text[:max_length-1] + "…"

def format_responsive_table(table_data, headers):
    """Format table to fit terminal width"""
    terminal_width = shutil.get_terminal_size().columns

    min_widths = [len(header) for header in headers]
    for row in table_data:
        for i, cell in enumerate(row):
            min_widths[i] = max(min_widths[i], len(str(cell)))

    table_overhead = 10
    available_width = terminal_width - table_overhead

    total_min_width = sum(min_widths)
    if total_min_width <= available_width:
        return tabulate(table_data, headers=headers, tablefmt="grid")

    # Find name column (always first column)
    name_col_idx = 0
    other_cols = list(range(1, len(headers)))
    fixed_width = sum(min_widths[i] for i in other_cols)

    name_width = available_width - fixed_width
    if name_width < 20:
        name_width = 20

    responsive_data = []
    for row in table_data:
        new_row = list(row)
        new_row[name_col_idx] = truncate_text(str(row[name_col_idx]), name_width)
        responsive_data.append(tuple(new_row))

    if available_width < 80:
        return tabulate(responsive_data, headers=headers, tablefmt="simple")
    else:
        return tabulate(responsive_data, headers=headers, tablefmt="grid")

def format_unified_table(items, show_type_column=False):
    """Smart table formatting based on content mix"""
    headers = ["Name", "Year", "TMDB Score", "Size", "Waste Score"]

    # Add Type column if showing mixed content
    if show_type_column:
        headers.insert(1, "Type")  # Insert after Name

    table_data = []
    total_size = 0
    total_waste_score = 0

    for item in items:
        row = [item.name, item.year, item.rating,
               format_file_size(item.size_bytes), item.waste_score]

        if show_type_column:
            row.insert(1, item.item_type.title())  # Insert type

        table_data.append(row)
        total_size += item.size_bytes
        total_waste_score += item.waste_score

    # Add totals row
    if items:
        # Calculate statistics
        item_count = len(items)
        avg_waste_score = round(total_waste_score / item_count)

        # Extract numeric ratings (excluding "N/A")
        numeric_ratings = []
        for item in items:
            if item.rating != "N/A":
                try:
                    numeric_ratings.append(float(item.rating))
                except (ValueError, TypeError):
                    pass

        # Calculate rating statistics
        if numeric_ratings:
            avg_rating = sum(numeric_ratings) / len(numeric_ratings)
            try:
                rating_mode = mode(numeric_ratings)
            except:  # No unique mode
                rating_mode = avg_rating
            rating_median = median(numeric_ratings)
            rating_display = f"{avg_rating:.1f} ({rating_mode:.1f}/{rating_median:.1f})"
        else:
            rating_display = "N/A"

        # Count distinct item types
        item_types = set(item.item_type for item in items)
        type_count = len(item_types)
        type_display = f"{type_count} type{'s' if type_count != 1 else ''}"

        # Build total row
        total_row = [f"Total ({item_count})", "", rating_display, format_file_size(total_size), avg_waste_score]

        if show_type_column:
            total_row.insert(1, type_display)  # Insert type count

        table_data.append(total_row)

    return format_responsive_table(table_data, headers)

def scan_api_data(item_type, cache_stats, sonarr_cache, radarr_cache):
    """API-based scanner that yields Item objects from Sonarr/Radarr"""
    items = []

    if item_type == 'sonarr':
        # Fetch all series from Sonarr API
        series_list = get_sonarr_series()
        for series in series_list:
            try:
                series_id = series.get('id')
                title = series.get('title', 'Unknown')
                year = series.get('year', 0)
                
                # Extract size from statistics object
                statistics = series.get('statistics', {})
                size_bytes = statistics.get('sizeOnDisk', 0)
                
                # Extract rating from series data
                ratings = series.get('ratings', {})
                if isinstance(ratings, dict) and 'value' in ratings:
                    rating = f"{ratings['value']:.1f}" if ratings['value'] > 0 else "N/A"
                else:
                    rating = "N/A"

                # Handle cache
                cache_key = str(series_id)
                if sonarr_cache is not None:
                    if cache_key in sonarr_cache:
                        cache_stats['hits'] += 1
                        rating = sonarr_cache[cache_key]
                    else:
                        cache_stats['misses'] += 1
                        sonarr_cache[cache_key] = rating

                # Only include series with files by default
                if size_bytes > 0:
                    items.append(Item(title, year, size_bytes, rating, 'tv', series_id))
                    
            except (KeyError, TypeError, ValueError) as e:
                print(f"Skipping malformed series data: {e}")
                continue

    elif item_type == 'radarr':
        # Fetch all movies from Radarr API
        movies_list = get_radarr_movies()
        for movie in movies_list:
            try:
                movie_id = movie.get('id')
                title = movie.get('title', 'Unknown')
                year = movie.get('year', 0)
                size_bytes = movie.get('sizeOnDisk', 0)
                has_file = movie.get('hasFile', False)
                
                # Extract rating from movie data
                ratings = movie.get('ratings', {})
                if isinstance(ratings, dict) and 'tmdb' in ratings:
                    tmdb_rating = ratings['tmdb']
                    if isinstance(tmdb_rating, dict) and 'value' in tmdb_rating:
                        rating = f"{tmdb_rating['value']:.1f}" if tmdb_rating['value'] > 0 else "N/A"
                    else:
                        rating = "N/A"
                else:
                    rating = "N/A"

                # Handle cache
                cache_key = str(movie_id)
                if radarr_cache is not None:
                    if cache_key in radarr_cache:
                        cache_stats['hits'] += 1
                        rating = radarr_cache[cache_key]
                    else:
                        cache_stats['misses'] += 1
                        radarr_cache[cache_key] = rating

                # Only include movies with files by default
                if size_bytes > 0:
                    items.append(Item(title, year, size_bytes, rating, 'movie', movie_id))
                    
            except (KeyError, TypeError, ValueError) as e:
                print(f"Skipping malformed movie data: {e}")
                continue

    return items

def print_results(items, requested_types, args, min_size_bytes=None):
    """Unified result printing with context-aware messaging"""

    # Filter by minimum waste score if specified
    if args.waste_score:
        items = [item for item in items if item.waste_score >= args.waste_score]

    # Filter by minimum size if specified
    if min_size_bytes:
        items = [item for item in items if item.size_bytes >= min_size_bytes]

    # Filter to keep only low-rated items if specified
    if args.ratings:
        filtered_items = []
        for item in items:
            if item.rating == "N/A":
                filtered_items.append(item)  # Keep N/A ratings
                continue
            try:
                if float(item.rating) <= args.ratings:  # Keep only items with rating EQUAL TO OR LOWER than threshold
                    filtered_items.append(item)
            except (ValueError, TypeError):
                continue  # Skip invalid ratings
        items = filtered_items

    # Always sort by waste score (descending)
    items.sort(key=lambda x: x.waste_score, reverse=True)

    # Determine display context
    if len(requested_types) == 1:
        if requested_types[0] == 'sonarr':
            type_name = "Series"
            title_prefix = "Series with"
        elif requested_types[0] == 'radarr':
            type_name = "Movies"
            title_prefix = "Movies with"
        else:
            type_name = requested_types[0].title()
            title_prefix = f"{type_name}s with"
        show_type_column = False
    else:
        show_type_column = True
        title_prefix = "Items with"

    # Build title and limit results
    title_parts = []

    if args.waste_score:
        title_parts.append(f"Waste Score >= {args.waste_score}")

    if min_size_bytes:
        title_parts.append(f"Size >= {format_file_size(min_size_bytes)}")

    if args.ratings:
        title_parts.append(f"Rating <= {args.ratings}")

    if args.top_waste:
        items = items[:args.top_waste]
        if title_parts:
            title_parts.append(f"Top {args.top_waste}")
        else:
            title_parts.append(f"Top {args.top_waste} Highest Waste Scores")

    if title_parts:
        title_suffix = f" ({', '.join(title_parts)})"
        print(f"{title_prefix} {title_parts[0] if len(title_parts) == 1 else 'Highest Waste Scores'}{title_suffix}")
        print("=" * 60)

    print(format_unified_table(items, show_type_column))

    # Summary stats
    if len(requested_types) > 1:
        tv_count = sum(1 for item in items if item.item_type == 'tv')
        movie_count = sum(1 for item in items if item.item_type == 'movie')
        print(f"\nTotal items: {len(items)} ({tv_count} series, {movie_count} movies)")
    else:
        if requested_types[0] == 'sonarr':
            print(f"\nTotal series shown: {len(items)}")
        elif requested_types[0] == 'radarr':
            print(f"\nTotal movies shown: {len(items)}")
        else:
            type_name = requested_types[0]
            print(f"\nTotal {type_name}s shown: {len(items)}")

def main():
    parser = argparse.ArgumentParser(description="Analyze Sonarr/Radarr collections with ratings and waste scores")
    parser.add_argument("item_type", nargs='?', choices=["sonarr", "radarr"],
                       help="Type of items to analyze: 'sonarr' for TV series, 'radarr' for movies (default: both)")
    parser.add_argument("--top-waste", "-t", type=int, metavar="N",
                       help="Show only the N items with highest waste scores")
    parser.add_argument("--waste-score", "-s", type=int, metavar="SCORE",
                       help="Show only items with waste score >= SCORE")
    parser.add_argument("--min-size", "-m", type=str, metavar="SIZE",
                       help="Show only items with size >= SIZE (e.g., 12M, 3GB, 500MB)")
    parser.add_argument("--ratings", "-r", type=float, metavar="RATING",
                       help="Show only items with rating <= RATING (e.g., 6.2, 7.5)")
    parser.add_argument("--clear-cache", action="store_true",
                       help="Clear cache and regenerate all ratings")
    parser.add_argument("--no-cache", action="store_true",
                       help="Bypass cache entirely (slower but always fresh)")

    args = parser.parse_args()

    # Handle cache clearing
    if args.clear_cache:
        if CACHE_FILE.exists():
            print(f"Clearing cache: {CACHE_FILE}")
            CACHE_FILE.unlink()
        else:
            print("No cache file to clear")

    # Parse min-size if provided
    min_size_bytes = None
    if args.min_size:
        try:
            min_size_bytes = parse_size_string(args.min_size)
        except ValueError as e:
            print(f"Error: {e}")
            return 1

    # Determine what to scan
    if args.item_type:
        scan_types = [args.item_type]
    else:
        scan_types = ["sonarr", "radarr"]  # Scan both

    # Validate API connectivity
    api_errors = validate_api_connectivity(scan_types)
    if api_errors:
        print("Error: API connectivity issues detected:")
        for error in api_errors:
            print(f"  - {error}")
        print("\nPlease ensure:")
        print("  - Sonarr/Radarr services are running")
        print("  - API keys are correctly set via environment variables")
        print("  - URLs are accessible")
        return 1

    # Load cache once at the beginning (unless bypassing cache)
    if args.no_cache:
        print("Bypassing cache - fetching fresh ratings")
        cache = {'sonarr': {}, 'radarr': {}}
        sonarr_cache = None  # Signal to skip cache
        radarr_cache = None
    else:
        cache = load_cache()
        sonarr_cache = cache['sonarr']
        radarr_cache = cache['radarr']

    # Process all requested types
    all_items = []
    cache_stats = {'hits': 0, 'misses': 0}

    for scan_type in scan_types:
        print(f"Fetching {scan_type} data from API")
        items = scan_api_data(scan_type, cache_stats, sonarr_cache, radarr_cache)
        all_items.extend(items)

    # Save cache once at the end (unless bypassing cache)
    if not args.no_cache:
        save_cache(sonarr_cache, radarr_cache)

    # Apply waste scoring and display
    print(f"Processing {len(all_items)} items")

    for item in all_items:
        item.waste_score = calculate_normalized_waste_score(item)

    print_results(all_items, scan_types, args, min_size_bytes)

    # Cache stats
    if cache_stats['hits'] > 0 or cache_stats['misses'] > 0:
        print(f"Cache stats: {cache_stats['hits']} hits, {cache_stats['misses']} misses")

if __name__ == "__main__":
    main()
