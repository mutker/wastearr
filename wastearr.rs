use anyhow::{Context, Result};
use clap::{Arg, ArgAction, Command};
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
    item_type: String, // 'show' or 'movie'
    waste_score: i32,
}

#[derive(Debug)]
struct Config {
    sonarr_url: String,
    sonarr_api_key: Option<String>,
    radarr_url: String,
    radarr_api_key: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheData {
    timestamp: f64,
    sonarr_ratings: HashMap<String, String>,
    radarr_ratings: HashMap<String, String>,
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

fn load_file_vars(file_path: &Path) -> HashMap<String, String> {
    fs::read_to_string(file_path).map_or_else(
        |_| HashMap::new(),
        |contents| {
            contents
                .lines()
                .filter_map(|line| {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') || !line.contains('=') {
                        return None;
                    }
                    line.split_once('=').map(|(key, value)| {
                        let key = key.trim().to_string();
                        let value = value
                            .trim()
                            .strip_prefix('"')
                            .unwrap_or(value.trim())
                            .strip_suffix('"')
                            .unwrap_or(value.trim())
                            .strip_prefix('\'')
                            .unwrap_or(value.trim())
                            .strip_suffix('\'')
                            .unwrap_or(value.trim())
                            .to_string();
                        (key, value)
                    })
                })
                .collect()
        },
    )
}

fn get_config_value(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .or_else(|| {
            config_dir().and_then(|dir| {
                load_file_vars(&dir.join("wastearr/config"))
                    .get(key)
                    .cloned()
            })
        })
        .or_else(|| load_file_vars(&PathBuf::from(".env")).get(key).cloned())
        .or_else(|| {
            load_file_vars(&PathBuf::from("/etc/wastearr/config"))
                .get(key)
                .cloned()
        })
}

fn fetch_api_data(
    base_url: &str,
    api_key: &str,
    endpoint: &str,
    service_name: &str,
) -> Result<Vec<Value>> {
    let url = format!("{}/api/v3/{}", base_url, endpoint);
    let response = Client::new()
        .get(&url)
        .header("X-Api-Key", api_key)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .with_context(|| format!("Failed to connect to {} API", service_name))?;

    if response.status().is_success() {
        let data: Vec<Value> = response
            .json()
            .with_context(|| format!("Failed to parse {} API response", service_name))?;
        println!(
            "Fetched {} {}s from {} API",
            data.len(),
            endpoint,
            service_name
        );
        Ok(data)
    } else {
        anyhow::bail!(
            "Failed to fetch {}s from {} API: HTTP {}",
            endpoint,
            service_name,
            response.status()
        )
    }
}

fn scan_api_data(
    base_url: &str,
    api_key: Option<&String>,
    endpoint: &str,
    service_name: &str,
    item_type: &str,
    cache_stats: &mut (usize, usize),
    cache: &mut Option<&mut HashMap<String, String>>,
) -> Result<Vec<Item>> {
    let api_key = api_key.with_context(|| {
        format!(
            "{}_API_KEY environment variable not set",
            service_name.to_uppercase()
        )
    })?;
    let data = fetch_api_data(base_url, api_key, endpoint, service_name)?;

    Ok(data
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_i64()? as i32;
            let title = item.get("title")?.as_str()?.to_string();
            let year = item.get("year")?.as_i64()? as i32;

            let size_bytes = if item_type == "show" {
                item.get("statistics")?.get("sizeOnDisk")?.as_u64()?
            } else {
                item.get("sizeOnDisk")?.as_u64()?
            };

            if size_bytes == 0 {
                return None;
            }

            let mut rating = item
                .get("ratings")
                .and_then(|r| {
                    if item_type == "show" {
                        r.get("value")
                    } else {
                        r.get("tmdb")?.get("value")
                    }
                })
                .and_then(|v| v.as_f64())
                .filter(|&r| r > 0.0)
                .map(|r| format!("{:.1}", r))
                .unwrap_or_else(|| "N/A".to_string());

            let cache_key = id.to_string();
            if let Some(cache_ref) = cache {
                if let Some(cached_rating) = cache_ref.get(&cache_key) {
                    cache_stats.0 += 1;
                    rating = cached_rating.clone();
                } else {
                    cache_stats.1 += 1;
                    cache_ref.insert(cache_key, rating.clone());
                }
            }

            Some(Item {
                name: title,
                year,
                size_bytes,
                rating,
                item_type: item_type.to_string(),
                waste_score: 0,
            })
        })
        .collect())
}

fn validate_api_connectivity(config: &Config, scan_types: &[String]) -> Result<()> {
    let client = Client::new();
    let api_errors: Vec<String> = scan_types
        .iter()
        .filter_map(|scan_type| {
            let (url, api_key, service_name) = match scan_type.as_str() {
                "sonarr" => (&config.sonarr_url, config.sonarr_api_key.as_ref(), "Sonarr"),
                "radarr" => (&config.radarr_url, config.radarr_api_key.as_ref(), "Radarr"),
                _ => return None,
            };

            api_key.map_or(
                Some(format!(
                    "{}_API_KEY environment variable not set",
                    service_name.to_uppercase()
                )),
                |key| match client
                    .get(format!("{}/api/v3/system/status", url))
                    .header("X-Api-Key", key)
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                {
                    Ok(resp) if resp.status().is_success() => None,
                    Ok(resp) => Some(format!(
                        "{} API unreachable at {} (HTTP {})",
                        service_name,
                        url,
                        resp.status()
                    )),
                    Err(e) => Some(format!(
                        "Cannot connect to {} at {}: {}",
                        service_name, url, e
                    )),
                },
            )
        })
        .collect();

    if !api_errors.is_empty() {
        eprintln!("Error: API connectivity issues detected:");
        api_errors
            .iter()
            .for_each(|error| eprintln!("  - {}", error));
        eprintln!("\nPlease ensure:");
        eprintln!("  - Sonarr/Radarr services are running");
        eprintln!("  - API keys are correctly set via environment variables");
        eprintln!("  - URLs are accessible");
        anyhow::bail!("API connectivity validation failed");
    }

    Ok(())
}

fn load_cache() -> (HashMap<String, String>, HashMap<String, String>) {
    cache_dir()
        .and_then(|dir| {
            let cache_path = dir.join("wastearr/cache.json");
            if !cache_path.exists() {
                println!("No existing cache found");
                return None;
            }

            fs::read_to_string(&cache_path).ok().and_then(|contents| {
                serde_json::from_str::<CacheData>(&contents)
                    .ok()
                    .and_then(|cache_data| {
                        let current_time = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs_f64();
                        if current_time - cache_data.timestamp > CACHE_DURATION as f64 {
                            println!("Cache expired, removing old cache file");
                            let _ = fs::remove_file(&cache_path);
                            None
                        } else {
                            println!("Loading cache from {}", cache_path.display());
                            Some((cache_data.sonarr_ratings, cache_data.radarr_ratings))
                        }
                    })
                    .or_else(|| {
                        println!("Cache corrupted, starting fresh");
                        let _ = fs::remove_file(&cache_path);
                        None
                    })
            })
        })
        .unwrap_or_else(|| {
            if cache_dir().is_none() {
                println!("No cache directory available");
            }
            (HashMap::new(), HashMap::new())
        })
}

fn save_cache(sonarr_cache: &HashMap<String, String>, radarr_cache: &HashMap<String, String>) {
    if let Some(cache_path) = cache_dir().map(|d| d.join("wastearr/cache.json")) {
        let cache_data = CacheData {
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs_f64(),
            sonarr_ratings: sonarr_cache.clone(),
            radarr_ratings: radarr_cache.clone(),
        };
        println!(
            "Saving cache with {} ratings",
            sonarr_cache.len() + radarr_cache.len()
        );
        if let Some(parent) = cache_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(&cache_data) {
            let _ = fs::write(&cache_path, json);
        }
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

fn get_rating_multiplier(rating: f64, is_tv: bool) -> f64 {
    let multipliers = if is_tv {
        [0.05, 0.15, 0.35, 0.55, 0.75, 1.1] // TV: more forgiving
    } else {
        [0.1, 0.2, 0.4, 0.6, 0.8, 1.2] // Movies: stricter
    };

    let thresholds = [8.0, 7.5, 7.0, 6.5, 6.0];
    thresholds
        .iter()
        .position(|&threshold| rating >= threshold)
        .map(|i| multipliers[i])
        .unwrap_or(multipliers[5])
}

fn calculate_normalized_waste_score(item: &mut Item) {
    let rating = item.rating.parse::<f64>().unwrap_or(6.0);
    let base_size_score = calculate_size_score(item.size_bytes);
    let is_tv = item.item_type == "show";

    let normalized_size = if is_tv {
        base_size_score * 0.6
    } else {
        base_size_score
    };
    let waste_score = normalized_size * get_rating_multiplier(rating, is_tv);
    item.waste_score = (waste_score.round() as i32).clamp(0, 100);
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
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

fn mode(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut counts = HashMap::new();
    for &v in values {
        *counts.entry((v * 10.0).round() as i32).or_insert(0) += 1;
    }
    counts
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(&val, _)| val as f64 / 10.0)
        .unwrap_or(0.0)
}

fn format_unified_table(items: &[Item], show_type_column: bool) -> String {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS);

    let mut headers = vec!["Name", "Year", "TMDB Score", "Size", "Waste Score"];
    if show_type_column {
        headers.insert(1, "Type");
    }
    table.set_header(&headers);

    let (total_size, total_waste) = items.iter().fold((0u64, 0i32), |acc, item| {
        let mut row = vec![
            item.name.clone(),
            item.year.to_string(),
            item.rating.clone(),
            format_file_size(item.size_bytes),
            item.waste_score.to_string(),
        ];
        if show_type_column {
            row.insert(
                1,
                if item.item_type == "show" {
                    "Show"
                } else {
                    "Movie"
                }
                .to_string(),
            );
        }
        table.add_row(row);
        (acc.0 + item.size_bytes, acc.1 + item.waste_score)
    });

    if !items.is_empty() {
        let numeric_ratings: Vec<f64> = items
            .iter()
            .filter_map(|item| item.rating.parse().ok())
            .collect();
        let rating_display = if numeric_ratings.is_empty() {
            "N/A".to_string()
        } else {
            let avg = numeric_ratings.iter().sum::<f64>() / numeric_ratings.len() as f64;
            format!(
                "{:.1} ({:.1}/{:.1})",
                avg,
                mode(&numeric_ratings),
                median(numeric_ratings.clone())
            )
        };

        let mut total_row = vec![
            format!("Total ({})", items.len()),
            "".to_string(),
            rating_display,
            format_file_size(total_size),
            (total_waste / items.len() as i32).to_string(),
        ];
        if show_type_column {
            let types: std::collections::HashSet<_> = items.iter().map(|i| &i.item_type).collect();
            total_row.insert(
                1,
                format!(
                    "{} type{}",
                    types.len(),
                    if types.len() != 1 { "s" } else { "" }
                ),
            );
        }
        table.add_row(total_row);
    }

    table.to_string()
}

fn parse_args() -> Args {
    let matches = Command::new("wastearr")
        .about("Analyze Sonarr/Radarr collections with ratings and waste scores")
        .arg(Arg::new("item_type").value_parser(["sonarr", "radarr"]))
        .arg(
            Arg::new("top-waste")
                .short('t')
                .long("top-waste")
                .value_parser(clap::value_parser!(usize)),
        )
        .arg(
            Arg::new("waste-score")
                .short('s')
                .long("waste-score")
                .value_parser(clap::value_parser!(i32)),
        )
        .arg(Arg::new("min-size").short('m').long("min-size"))
        .arg(
            Arg::new("ratings")
                .short('r')
                .long("ratings")
                .value_parser(clap::value_parser!(f64)),
        )
        .arg(
            Arg::new("clear-cache")
                .long("clear-cache")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("no-cache")
                .long("no-cache")
                .action(ArgAction::SetTrue),
        )
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
    items.retain(|item| {
        args.waste_score.is_none_or(|min| item.waste_score >= min)
            && min_size_bytes.is_none_or(|min| item.size_bytes >= min)
            && args.ratings.is_none_or(|max| {
                item.rating == "N/A" || item.rating.parse::<f64>().unwrap_or(0.0) <= max
            })
    });

    items.sort_by_key(|item| std::cmp::Reverse(item.waste_score));

    let mut filters = Vec::new();
    if let Some(score) = args.waste_score {
        filters.push(format!("Waste Score >= {}", score));
    }
    if let Some(size) = min_size_bytes {
        filters.push(format!("Size >= {}", format_file_size(size)));
    }
    if let Some(rating) = args.ratings {
        filters.push(format!("Rating <= {}", rating));
    }

    if let Some(top_n) = args.top_waste {
        items.truncate(top_n);
        if filters.is_empty() {
            filters.push(format!("Top {} Highest Waste Scores", top_n));
        }
    }

    if !filters.is_empty() {
        let prefix = if requested_types.len() == 1 {
            match requested_types[0].as_str() {
                "sonarr" => "Series",
                "radarr" => "Movies",
                _ => "Items",
            }
        } else {
            "Items"
        };
        println!("{} with {}", prefix, filters.join(", "));
        println!("{}", "=".repeat(60));
    }

    println!("{}", format_unified_table(items, requested_types.len() > 1));

    if requested_types.len() > 1 {
        let (tv, movies) = items.iter().fold((0, 0), |acc, item| {
            if item.item_type == "show" {
                (acc.0 + 1, acc.1)
            } else {
                (acc.0, acc.1 + 1)
            }
        });
        println!(
            "\nTotal items: {} ({} series, {} movies)",
            items.len(),
            tv,
            movies
        );
    } else {
        let item_type = match requested_types[0].as_str() {
            "sonarr" => "series",
            "radarr" => "movies",
            _ => &requested_types[0],
        };
        println!("\nTotal {} shown: {}", item_type, items.len());
    }
}

fn main() -> Result<()> {
    let args = parse_args();
    let config = Config {
        sonarr_url: get_config_value("SONARR_URL")
            .unwrap_or_else(|| "http://localhost:8989".to_string()),
        sonarr_api_key: get_config_value("SONARR_API_KEY"),
        radarr_url: get_config_value("RADARR_URL")
            .unwrap_or_else(|| "http://localhost:7878".to_string()),
        radarr_api_key: get_config_value("RADARR_API_KEY"),
    };

    if args.clear_cache {
        if let Some(cache_path) = cache_dir().map(|d| d.join("wastearr/cache.json")) {
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
    let mut cache_stats = (0usize, 0usize); // (hits, misses)

    for scan_type in &scan_types {
        println!("Fetching {} data from API", scan_type);

        let items = match scan_type.as_str() {
            "sonarr" => {
                let mut cache_ref = if args.no_cache {
                    None
                } else {
                    Some(&mut sonarr_cache)
                };
                scan_api_data(
                    &config.sonarr_url,
                    config.sonarr_api_key.as_ref(),
                    "series",
                    "Sonarr",
                    "show",
                    &mut cache_stats,
                    &mut cache_ref,
                )?
            }
            "radarr" => {
                let mut cache_ref = if args.no_cache {
                    None
                } else {
                    Some(&mut radarr_cache)
                };
                scan_api_data(
                    &config.radarr_url,
                    config.radarr_api_key.as_ref(),
                    "movie",
                    "Radarr",
                    "movie",
                    &mut cache_stats,
                    &mut cache_ref,
                )?
            }
            _ => Vec::new(),
        };

        all_items.extend(items);
    }

    if !args.no_cache {
        save_cache(&sonarr_cache, &radarr_cache);
    }

    println!("Processing {} items", all_items.len());
    all_items
        .iter_mut()
        .for_each(calculate_normalized_waste_score);

    print_results(&mut all_items, &scan_types, &args, min_size_bytes);

    if cache_stats.0 > 0 || cache_stats.1 > 0 {
        println!(
            "Cache stats: {} hits, {} misses",
            cache_stats.0, cache_stats.1
        );
    }

    Ok(())
}
