use anyhow::{Context, Result};
use clap::{Arg, ArgAction, Command, ValueHint};
use comfy_table::{Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL};
use dirs::{cache_dir, config_dir};
use regex::Regex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const CACHE_DURATION: u64 = 72 * 60 * 60; // 72 hours in seconds

#[derive(Debug, Clone)]
struct Item {
    name: String,
    year: i32,
    size_bytes: u64,
    rating: String,
    item_type: String, // 'tv' or 'movie'
    identifier: Option<i32>,
    waste_score: i32,
}

impl Item {
    fn new(
        name: String,
        year: i32,
        size_bytes: u64,
        rating: String,
        item_type: String,
        identifier: Option<i32>,
    ) -> Self {
        Item {
            name,
            year,
            size_bytes,
            rating,
            item_type,
            identifier,
            waste_score: 0,
        }
    }
}

#[derive(Debug)]
struct Config {
    sonarr_url: String,
    sonarr_api_key: Option<String>,
    radarr_url: String,
    radarr_api_key: Option<String>,
}

impl Config {
    fn new() -> Result<Self> {
        Ok(Config {
            sonarr_url: get_config_value("SONARR_URL")
                .unwrap_or_else(|| "http://localhost:8989".to_string()),
            sonarr_api_key: get_config_value("SONARR_API_KEY"),
            radarr_url: get_config_value("RADARR_URL")
                .unwrap_or_else(|| "http://localhost:7878".to_string()),
            radarr_api_key: get_config_value("RADARR_API_KEY"),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheData {
    timestamp: f64,
    sonarr_ratings: HashMap<String, String>,
    radarr_ratings: HashMap<String, String>,
}

impl CacheData {
    fn new() -> Self {
        CacheData {
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs_f64(),
            sonarr_ratings: HashMap::new(),
            radarr_ratings: HashMap::new(),
        }
    }

    fn is_expired(&self) -> bool {
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        current_time - self.timestamp > CACHE_DURATION as f64
    }
}

#[derive(Debug)]
struct Args {
    item_type: Option<String>,
    top_waste: Option<usize>,
    waste_score: Option<i32>,
    min_size: Option<String>,
    ratings: Option<f64>,
    clear_cache: bool,
    no_cache: bool,
}

#[derive(Debug)]
struct CacheStats {
    hits: usize,
    misses: usize,
}

impl CacheStats {
    fn new() -> Self {
        CacheStats { hits: 0, misses: 0 }
    }
}

fn load_env_file(env_file_path: &Path) -> HashMap<String, String> {
    let mut env_vars = HashMap::new();
    if let Ok(contents) = fs::read_to_string(env_file_path) {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || !line.contains('=') {
                continue;
            }

            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value
                    .trim()
                    .strip_prefix('"')
                    .unwrap_or(value.trim())
                    .strip_suffix('"')
                    .unwrap_or(value.trim())
                    .strip_prefix('\'')
                    .unwrap_or(value.trim())
                    .strip_suffix('\'')
                    .unwrap_or(value.trim());
                env_vars.insert(key.to_string(), value.to_string());
            }
        }
    }
    env_vars
}

fn load_config_file(config_file_path: &Path) -> HashMap<String, String> {
    let mut config_vars = HashMap::new();
    if let Ok(contents) = fs::read_to_string(config_file_path) {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || !line.contains('=') {
                continue;
            }

            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value
                    .trim()
                    .strip_prefix('"')
                    .unwrap_or(value.trim())
                    .strip_suffix('"')
                    .unwrap_or(value.trim())
                    .strip_prefix('\'')
                    .unwrap_or(value.trim())
                    .strip_suffix('\'')
                    .unwrap_or(value.trim());
                config_vars.insert(key.to_string(), value.to_string());
            }
        }
    }
    config_vars
}

fn get_config_value(key: &str) -> Option<String> {
    // 1. Check environment variables first
    if let Ok(value) = env::var(key) {
        return Some(value);
    }

    // 2. Check .env file in current directory
    let env_file = PathBuf::from(".env");
    let env_vars = load_env_file(&env_file);
    if let Some(value) = env_vars.get(key) {
        return Some(value.clone());
    }

    // 3. Check user config directory
    if let Some(config_dir) = config_dir() {
        let config_file = config_dir.join("wastearr").join("conf");
        let config_vars = load_config_file(&config_file);
        if let Some(value) = config_vars.get(key) {
            return Some(value.clone());
        }
    }

    // 4. Return None if not found
    None
}

fn get_sonarr_series(config: &Config) -> Result<Vec<Value>> {
    let api_key = config
        .sonarr_api_key
        .as_ref()
        .context("SONARR_API_KEY environment variable not set")?;

    let client = Client::new();
    let url = format!("{}/api/v3/series", config.sonarr_url);

    let response = client
        .get(&url)
        .header("X-Api-Key", api_key)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .context("Failed to connect to Sonarr API")?;

    if response.status().is_success() {
        let series_list: Vec<Value> = response
            .json()
            .context("Failed to parse Sonarr API response")?;
        println!("Fetched {} series from Sonarr API", series_list.len());
        Ok(series_list)
    } else {
        anyhow::bail!(
            "Failed to fetch series from Sonarr API: HTTP {}",
            response.status()
        );
    }
}

fn scan_sonarr_data(
    config: &Config,
    cache_stats: &mut CacheStats,
    sonarr_cache: &mut Option<&mut HashMap<String, String>>,
) -> Result<Vec<Item>> {
    let series_list = get_sonarr_series(config)?;
    let mut items = Vec::new();

    for series in series_list {
        let series_id = series.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

        let title = series
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();

        let year = series.get("year").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

        // Extract size from statistics object
        let size_bytes = series
            .get("statistics")
            .and_then(|stats| stats.get("sizeOnDisk"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Extract rating from series data
        let mut rating = "N/A".to_string();
        if let Some(ratings) = series.get("ratings") {
            if let Some(value) = ratings.get("value") {
                if let Some(rating_value) = value.as_f64() {
                    if rating_value > 0.0 {
                        rating = format!("{:.1}", rating_value);
                    }
                }
            }
        }

        // Handle cache
        let cache_key = series_id.to_string();
        if let Some(cache) = sonarr_cache {
            if let Some(cached_rating) = cache.get(&cache_key) {
                cache_stats.hits += 1;
                rating = cached_rating.clone();
            } else {
                cache_stats.misses += 1;
                cache.insert(cache_key, rating.clone());
            }
        }

        // Only include series with files by default
        if size_bytes > 0 {
            items.push(Item::new(
                title,
                year,
                size_bytes,
                rating,
                "tv".to_string(),
                Some(series_id),
            ));
        }
    }

    Ok(items)
}

fn get_radarr_movies(config: &Config) -> Result<Vec<Value>> {
    let api_key = config
        .radarr_api_key
        .as_ref()
        .context("RADARR_API_KEY environment variable not set")?;

    let client = Client::new();
    let url = format!("{}/api/v3/movie", config.radarr_url);

    let response = client
        .get(&url)
        .header("X-Api-Key", api_key)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .context("Failed to connect to Radarr API")?;

    if response.status().is_success() {
        let movies_list: Vec<Value> = response
            .json()
            .context("Failed to parse Radarr API response")?;
        println!("Fetched {} movies from Radarr API", movies_list.len());
        Ok(movies_list)
    } else {
        anyhow::bail!(
            "Failed to fetch movies from Radarr API: HTTP {}",
            response.status()
        );
    }
}

fn scan_radarr_data(
    config: &Config,
    cache_stats: &mut CacheStats,
    radarr_cache: &mut Option<&mut HashMap<String, String>>,
) -> Result<Vec<Item>> {
    let movies_list = get_radarr_movies(config)?;
    let mut items = Vec::new();

    for movie in movies_list {
        let movie_id = movie.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

        let title = movie
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();

        let year = movie.get("year").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

        let size_bytes = movie
            .get("sizeOnDisk")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Extract rating from movie data
        let mut rating = "N/A".to_string();
        if let Some(ratings) = movie.get("ratings") {
            if let Some(tmdb) = ratings.get("tmdb") {
                if let Some(value) = tmdb.get("value") {
                    if let Some(rating_value) = value.as_f64() {
                        if rating_value > 0.0 {
                            rating = format!("{:.1}", rating_value);
                        }
                    }
                }
            }
        }

        // Handle cache
        let cache_key = movie_id.to_string();
        if let Some(cache) = radarr_cache {
            if let Some(cached_rating) = cache.get(&cache_key) {
                cache_stats.hits += 1;
                rating = cached_rating.clone();
            } else {
                cache_stats.misses += 1;
                cache.insert(cache_key, rating.clone());
            }
        }

        // Only include movies with files by default
        if size_bytes > 0 {
            items.push(Item::new(
                title,
                year,
                size_bytes,
                rating,
                "movie".to_string(),
                Some(movie_id),
            ));
        }
    }

    Ok(items)
}

fn validate_api_connectivity(config: &Config, scan_types: &[String]) -> Result<()> {
    let mut api_errors = Vec::new();
    let client = Client::new();

    for scan_type in scan_types {
        match scan_type.as_str() {
            "sonarr" => {
                if config.sonarr_api_key.is_none() {
                    api_errors.push("SONARR_API_KEY environment variable not set".to_string());
                    continue;
                }

                let url = format!("{}/api/v3/system/status", config.sonarr_url);
                let response = client
                    .get(&url)
                    .header("X-Api-Key", config.sonarr_api_key.as_ref().unwrap())
                    .timeout(std::time::Duration::from_secs(5))
                    .send();

                match response {
                    Ok(resp) if resp.status().is_success() => {
                        // API is accessible
                    }
                    Ok(resp) => {
                        api_errors.push(format!(
                            "Sonarr API unreachable at {} (HTTP {})",
                            config.sonarr_url,
                            resp.status()
                        ));
                    }
                    Err(e) => {
                        api_errors.push(format!(
                            "Cannot connect to Sonarr at {}: {}",
                            config.sonarr_url, e
                        ));
                    }
                }
            }
            "radarr" => {
                if config.radarr_api_key.is_none() {
                    api_errors.push("RADARR_API_KEY environment variable not set".to_string());
                    continue;
                }

                let url = format!("{}/api/v3/system/status", config.radarr_url);
                let response = client
                    .get(&url)
                    .header("X-Api-Key", config.radarr_api_key.as_ref().unwrap())
                    .timeout(std::time::Duration::from_secs(5))
                    .send();

                match response {
                    Ok(resp) if resp.status().is_success() => {
                        // API is accessible
                    }
                    Ok(resp) => {
                        api_errors.push(format!(
                            "Radarr API unreachable at {} (HTTP {})",
                            config.radarr_url,
                            resp.status()
                        ));
                    }
                    Err(e) => {
                        api_errors.push(format!(
                            "Cannot connect to Radarr at {}: {}",
                            config.radarr_url, e
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    if !api_errors.is_empty() {
        eprintln!("Error: API connectivity issues detected:");
        for error in &api_errors {
            eprintln!("  - {}", error);
        }
        eprintln!("\nPlease ensure:");
        eprintln!("  - Sonarr/Radarr services are running");
        eprintln!("  - API keys are correctly set via environment variables");
        eprintln!("  - URLs are accessible");
        anyhow::bail!("API connectivity validation failed");
    }

    Ok(())
}

fn get_cache_file_path() -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join("wastearr").join("cache.json"))
}

fn load_cache() -> (HashMap<String, String>, HashMap<String, String>) {
    let cache_file_path = match get_cache_file_path() {
        Some(path) => path,
        None => {
            println!("No cache directory available");
            return (HashMap::new(), HashMap::new());
        }
    };

    if !cache_file_path.exists() {
        println!("No existing cache found");
        return (HashMap::new(), HashMap::new());
    }

    match fs::read_to_string(&cache_file_path) {
        Ok(contents) => {
            match serde_json::from_str::<CacheData>(&contents) {
                Ok(cache_data) => {
                    if cache_data.is_expired() {
                        println!("Cache expired, removing old cache file");
                        let _ = fs::remove_file(&cache_file_path);
                        return (HashMap::new(), HashMap::new());
                    }

                    println!("Loading cache from {}", cache_file_path.display());
                    (cache_data.sonarr_ratings, cache_data.radarr_ratings)
                }
                Err(_) => {
                    // Try to parse old cache format for backward compatibility
                    if let Ok(old_cache) = serde_json::from_str::<Value>(&contents) {
                        if let Some(timestamp) = old_cache.get("timestamp").and_then(|t| t.as_f64())
                        {
                            let current_time = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_secs_f64();

                            if current_time - timestamp > CACHE_DURATION as f64 {
                                println!("Cache expired, removing old cache file");
                                let _ = fs::remove_file(&cache_file_path);
                                return (HashMap::new(), HashMap::new());
                            }

                            // Support old cache format migration
                            let sonarr_cache = old_cache
                                .get("sonarr_ratings")
                                .or_else(|| old_cache.get("tv_ratings"))
                                .and_then(|v| serde_json::from_value(v.clone()).ok())
                                .unwrap_or_else(HashMap::new);

                            let radarr_cache = old_cache
                                .get("radarr_ratings")
                                .or_else(|| old_cache.get("movie_ratings"))
                                .and_then(|v| serde_json::from_value(v.clone()).ok())
                                .unwrap_or_else(HashMap::new);

                            return (sonarr_cache, radarr_cache);
                        }
                    }

                    println!("Cache corrupted, starting fresh");
                    let _ = fs::remove_file(&cache_file_path);
                    (HashMap::new(), HashMap::new())
                }
            }
        }
        Err(_) => {
            println!("Cache corrupted, starting fresh");
            let _ = fs::remove_file(&cache_file_path);
            (HashMap::new(), HashMap::new())
        }
    }
}

fn save_cache(sonarr_cache: &HashMap<String, String>, radarr_cache: &HashMap<String, String>) {
    let cache_file_path = match get_cache_file_path() {
        Some(path) => path,
        None => return,
    };

    let cache_data = CacheData {
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64(),
        sonarr_ratings: sonarr_cache.clone(),
        radarr_ratings: radarr_cache.clone(),
    };

    let total_ratings = sonarr_cache.len() + radarr_cache.len();
    println!("Saving cache with {} ratings", total_ratings);

    if let Some(parent) = cache_file_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    if let Ok(json) = serde_json::to_string(&cache_data) {
        let _ = fs::write(&cache_file_path, json);
    }
}

fn calculate_size_score(size_bytes: u64) -> f64 {
    let size_gb = size_bytes as f64 / (1024.0_f64.powi(3));

    if size_gb <= 1.0 {
        size_gb * 10.0
    } else {
        10.0 + (size_gb.log10() * 30.0)
    }
    .min(80.0)
}

fn get_tv_rating_multiplier(rating: f64) -> f64 {
    if rating >= 8.0 {
        0.05 // Excellent shows: 95% penalty reduction
    } else if rating >= 7.5 {
        0.15 // Very good shows: 85% penalty reduction
    } else if rating >= 7.0 {
        0.35 // Good shows: 65% penalty reduction
    } else if rating >= 6.5 {
        0.55 // Decent shows: 45% penalty reduction
    } else if rating >= 6.0 {
        0.75 // Average shows: 25% penalty reduction
    } else {
        1.1 // Poor shows: 10% penalty increase
    }
}

fn get_movie_rating_multiplier(rating: f64) -> f64 {
    if rating >= 8.0 {
        0.1 // Excellent movies: 90% penalty reduction
    } else if rating >= 7.5 {
        0.2 // Very good movies: 80% penalty reduction
    } else if rating >= 7.0 {
        0.4 // Good movies: 60% penalty reduction
    } else if rating >= 6.5 {
        0.6 // Decent movies: 40% penalty reduction
    } else if rating >= 6.0 {
        0.8 // Average movies: 20% penalty reduction
    } else {
        1.2 // Poor movies: 20% penalty increase
    }
}

fn calculate_normalized_waste_score(item: &mut Item) {
    let rating = if item.rating == "N/A" {
        6.0
    } else {
        item.rating.parse::<f64>().unwrap_or(6.0)
    };

    // Base size score (same logarithmic scaling)
    let base_size_score = calculate_size_score(item.size_bytes);

    // Content-type normalization
    let (normalized_size, rating_multiplier) = if item.item_type == "tv" {
        // TV shows are expected to be larger (multi-season content)
        let normalized_size = base_size_score * 0.6; // 40% size discount
        let rating_multiplier = get_tv_rating_multiplier(rating);
        (normalized_size, rating_multiplier)
    } else {
        // movies
        // Movies should be more size-efficient
        let normalized_size = base_size_score * 1.0; // No size adjustment
        let rating_multiplier = get_movie_rating_multiplier(rating);
        (normalized_size, rating_multiplier)
    };

    let waste_score = normalized_size * rating_multiplier;
    item.waste_score = (waste_score.round() as i32).max(0).min(100);
}

fn format_file_size(size_bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut size = size_bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < units.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.1} {}", size, units[unit_index])
}

fn parse_size_string(size_str: &str) -> Result<u64> {
    let re = Regex::new(r"^(\d+(?:\.\d+)?)\s*([KMGTB]?B?)?$").unwrap();
    let size_upper = size_str.to_uppercase();

    let captures = re
        .captures(&size_upper)
        .context(format!("Invalid size format: {}", size_str))?;

    let number: f64 = captures
        .get(1)
        .unwrap()
        .as_str()
        .parse()
        .context("Invalid number in size string")?;

    let unit = captures.get(2).map(|m| m.as_str()).unwrap_or("B");

    let multiplier = match unit {
        "B" => 1,
        "K" | "KB" => 1024,
        "M" | "MB" => 1024_u64.pow(2),
        "G" | "GB" => 1024_u64.pow(3),
        "T" | "TB" => 1024_u64.pow(4),
        _ => anyhow::bail!("Unknown unit: {}", unit),
    };

    Ok((number * multiplier as f64) as u64)
}

fn median(mut values: Vec<f64>) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let len = values.len();
    if len % 2 == 0 {
        (values[len / 2 - 1] + values[len / 2]) / 2.0
    } else {
        values[len / 2]
    }
}

fn mode(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }

    let mut counts = HashMap::new();
    for &value in values {
        *counts.entry((value * 10.0).round() as i32).or_insert(0) += 1;
    }

    let most_frequent = counts
        .iter()
        .max_by_key(|&(_, count)| count)
        .map(|(val, _)| *val as f64 / 10.0)
        .unwrap_or(0.0);

    most_frequent
}

fn truncate_text(text: &str, max_length: usize) -> String {
    if text.len() <= max_length {
        text.to_string()
    } else {
        format!("{}…", &text[..max_length.saturating_sub(1)])
    }
}

fn format_unified_table(items: &[Item], show_type_column: bool) -> String {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS);

    // Build headers
    let mut headers = vec!["Name", "Year", "TMDB Score", "Size", "Waste Score"];
    if show_type_column {
        headers.insert(1, "Type");
    }
    table.set_header(&headers);

    // Add data rows
    let mut total_size = 0u64;
    let mut total_waste_score = 0i32;

    for item in items {
        let mut row = vec![
            item.name.clone(),
            item.year.to_string(),
            item.rating.clone(),
            format_file_size(item.size_bytes),
            item.waste_score.to_string(),
        ];

        if show_type_column {
            let item_type = if item.item_type == "tv" {
                "Tv"
            } else {
                "Movie"
            };
            row.insert(1, item_type.to_string());
        }

        table.add_row(row);
        total_size += item.size_bytes;
        total_waste_score += item.waste_score;
    }

    // Add totals row if we have items
    if !items.is_empty() {
        let item_count = items.len();
        let avg_waste_score = total_waste_score / item_count as i32;

        // Extract numeric ratings (excluding "N/A")
        let numeric_ratings: Vec<f64> = items
            .iter()
            .filter_map(|item| {
                if item.rating != "N/A" {
                    item.rating.parse::<f64>().ok()
                } else {
                    None
                }
            })
            .collect();

        // Calculate rating statistics
        let rating_display = if !numeric_ratings.is_empty() {
            let avg_rating = numeric_ratings.iter().sum::<f64>() / numeric_ratings.len() as f64;
            let rating_mode = mode(&numeric_ratings);
            let rating_median = median(numeric_ratings);
            format!(
                "{:.1} ({:.1}/{:.1})",
                avg_rating, rating_mode, rating_median
            )
        } else {
            "N/A".to_string()
        };

        // Count distinct item types
        let item_types: std::collections::HashSet<&String> =
            items.iter().map(|item| &item.item_type).collect();
        let type_count = item_types.len();
        let type_display = format!(
            "{} type{}",
            type_count,
            if type_count != 1 { "s" } else { "" }
        );

        // Build total row
        let mut total_row = vec![
            format!("Total ({})", item_count),
            "".to_string(),
            rating_display,
            format_file_size(total_size),
            avg_waste_score.to_string(),
        ];

        if show_type_column {
            total_row.insert(1, type_display);
        }

        table.add_row(total_row);
    }

    table.to_string()
}

fn parse_args() -> Args {
    let matches = Command::new("wastearr")
        .about("Analyze Sonarr/Radarr collections with ratings and waste scores")
        .arg(Arg::new("item_type")
            .help("Type of items to analyze: 'sonarr' for TV series, 'radarr' for movies (default: both)")
            .value_parser(["sonarr", "radarr"])
            .required(false))
        .arg(Arg::new("top-waste")
            .short('t')
            .long("top-waste")
            .help("Show only the N items with highest waste scores")
            .value_name("N")
            .value_parser(clap::value_parser!(usize)))
        .arg(Arg::new("waste-score")
            .short('s')
            .long("waste-score")
            .help("Show only items with waste score >= SCORE")
            .value_name("SCORE")
            .value_parser(clap::value_parser!(i32)))
        .arg(Arg::new("min-size")
            .short('m')
            .long("min-size")
            .help("Show only items with size >= SIZE (e.g., 12M, 3GB, 500MB)")
            .value_name("SIZE"))
        .arg(Arg::new("ratings")
            .short('r')
            .long("ratings")
            .help("Show only items with rating <= RATING (e.g., 6.2, 7.5)")
            .value_name("RATING")
            .value_parser(clap::value_parser!(f64)))
        .arg(Arg::new("clear-cache")
            .long("clear-cache")
            .help("Clear cache and regenerate all ratings")
            .action(ArgAction::SetTrue))
        .arg(Arg::new("no-cache")
            .long("no-cache")
            .help("Bypass cache entirely (slower but always fresh)")
            .action(ArgAction::SetTrue))
        .get_matches();

    Args {
        item_type: matches.get_one::<String>("item_type").cloned(),
        top_waste: matches.get_one::<usize>("top-waste").copied(),
        waste_score: matches.get_one::<i32>("waste-score").copied(),
        min_size: matches.get_one::<String>("min-size").cloned(),
        ratings: matches.get_one::<f64>("ratings").copied(),
        clear_cache: matches.get_flag("clear-cache"),
        no_cache: matches.get_flag("no-cache"),
    }
}

fn print_results(
    items: &mut Vec<Item>,
    requested_types: &[String],
    args: &Args,
    min_size_bytes: Option<u64>,
) {
    // Filter by minimum waste score if specified
    if let Some(min_score) = args.waste_score {
        items.retain(|item| item.waste_score >= min_score);
    }

    // Filter by minimum size if specified
    if let Some(min_size) = min_size_bytes {
        items.retain(|item| item.size_bytes >= min_size);
    }

    // Filter to keep only low-rated items if specified
    if let Some(max_rating) = args.ratings {
        items.retain(|item| {
            if item.rating == "N/A" {
                true // Keep N/A ratings
            } else {
                item.rating.parse::<f64>().unwrap_or(0.0) <= max_rating
            }
        });
    }

    // Always sort by waste score (descending)
    items.sort_by(|a, b| b.waste_score.cmp(&a.waste_score));

    // Determine display context
    let show_type_column = requested_types.len() > 1;
    let title_prefix = if requested_types.len() == 1 {
        match requested_types[0].as_str() {
            "sonarr" => "Series with",
            "radarr" => "Movies with",
            _ => "Items with",
        }
    } else {
        "Items with"
    };

    // Build title and limit results
    let mut title_parts = Vec::new();

    if let Some(min_score) = args.waste_score {
        title_parts.push(format!("Waste Score >= {}", min_score));
    }

    if let Some(min_size) = min_size_bytes {
        title_parts.push(format!("Size >= {}", format_file_size(min_size)));
    }

    if let Some(max_rating) = args.ratings {
        title_parts.push(format!("Rating <= {}", max_rating));
    }

    if let Some(top_n) = args.top_waste {
        items.truncate(top_n);
        if title_parts.is_empty() {
            title_parts.push(format!("Top {} Highest Waste Scores", top_n));
        } else {
            title_parts.push(format!("Top {}", top_n));
        }
    }

    if !title_parts.is_empty() {
        let main_title = if title_parts.len() == 1
            && title_parts[0].starts_with("Top")
            && title_parts[0].contains("Highest")
        {
            title_parts[0].clone()
        } else {
            title_parts[0].clone()
        };

        let title_suffix = format!(" ({})", title_parts.join(", "));
        println!(
            "{} {}{}",
            title_prefix,
            main_title,
            if title_parts.len() == 1
                && title_parts[0].starts_with("Top")
                && title_parts[0].contains("Highest")
            {
                ""
            } else {
                &title_suffix
            }
        );
        println!("{}", "=".repeat(60));
    }

    println!("{}", format_unified_table(items, show_type_column));

    // Summary stats
    if requested_types.len() > 1 {
        let tv_count = items.iter().filter(|item| item.item_type == "tv").count();
        let movie_count = items
            .iter()
            .filter(|item| item.item_type == "movie")
            .count();
        println!(
            "\nTotal items: {} ({} series, {} movies)",
            items.len(),
            tv_count,
            movie_count
        );
    } else {
        match requested_types[0].as_str() {
            "sonarr" => println!("\nTotal series shown: {}", items.len()),
            "radarr" => println!("\nTotal movies shown: {}", items.len()),
            _ => println!("\nTotal {}s shown: {}", requested_types[0], items.len()),
        }
    }
}

fn main() -> Result<()> {
    let args = parse_args();
    let config = Config::new()?;

    // Handle cache clearing
    if args.clear_cache {
        if let Some(cache_path) = get_cache_file_path() {
            if cache_path.exists() {
                println!("Clearing cache: {}", cache_path.display());
                fs::remove_file(&cache_path)?;
            } else {
                println!("No cache file to clear");
            }
        }
    }

    // Parse min-size if provided
    let min_size_bytes = if let Some(size_str) = &args.min_size {
        Some(parse_size_string(size_str)?)
    } else {
        None
    };

    // Determine what to scan
    let scan_types = if let Some(item_type) = &args.item_type {
        vec![item_type.clone()]
    } else {
        vec!["sonarr".to_string(), "radarr".to_string()]
    };

    // Validate API connectivity
    validate_api_connectivity(&config, &scan_types)?;

    // Load cache once at the beginning (unless bypassing cache)
    let (mut sonarr_cache, mut radarr_cache) = if args.no_cache {
        println!("Bypassing cache - fetching fresh ratings");
        (HashMap::new(), HashMap::new())
    } else {
        load_cache()
    };

    // Process all requested types
    let mut all_items = Vec::new();
    let mut cache_stats = CacheStats::new();

    for scan_type in &scan_types {
        println!("Fetching {} data from API", scan_type);

        let items = match scan_type.as_str() {
            "sonarr" => {
                let mut cache_ref = if args.no_cache {
                    None
                } else {
                    Some(&mut sonarr_cache)
                };
                scan_sonarr_data(&config, &mut cache_stats, &mut cache_ref)?
            }
            "radarr" => {
                let mut cache_ref = if args.no_cache {
                    None
                } else {
                    Some(&mut radarr_cache)
                };
                scan_radarr_data(&config, &mut cache_stats, &mut cache_ref)?
            }
            _ => Vec::new(),
        };

        all_items.extend(items);
    }

    // Save cache once at the end (unless bypassing cache)
    if !args.no_cache {
        save_cache(&sonarr_cache, &radarr_cache);
    }

    // Apply waste scoring and display
    println!("Processing {} items", all_items.len());

    for item in &mut all_items {
        calculate_normalized_waste_score(item);
    }

    print_results(&mut all_items, &scan_types, &args, min_size_bytes);

    // Cache stats
    if cache_stats.hits > 0 || cache_stats.misses > 0 {
        println!(
            "Cache stats: {} hits, {} misses",
            cache_stats.hits, cache_stats.misses
        );
    }

    Ok(())
}
