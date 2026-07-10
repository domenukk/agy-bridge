use std::{fs, path::PathBuf};

use agy_bridge::{
    config::{BuiltinTools, CapabilitiesConfig},
    content::{ContentPrimitive, Image},
    policies::PolicyRule,
    prelude::*,
};

fn header(title: &str) {
    println!("\n{}", "=".repeat(60));
    println!("  {title}");
    println!("{}", "=".repeat(60));
}

fn find_generated_image(name: &str) -> Option<PathBuf> {
    // NOLINT: example code — HOME not set means no search path, return None
    let home = std::env::var("HOME").ok()?;
    let base = PathBuf::from(home).join(".gemini/antigravity/brain");
    if !base.is_dir() {
        return None;
    }

    // Recursive search roughly similar to python's glob "**"
    let mut matches = Vec::new();
    let mut dirs_to_visit = vec![base];
    while let Some(d) = dirs_to_visit.pop() {
        match fs::read_dir(d) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        dirs_to_visit.push(path);
                    } else if let Some(n) = path.file_name().and_then(|n| n.to_str())
                        && n.starts_with(name)
                        && n.to_ascii_lowercase().ends_with(".png")
                    {
                        matches.push(path);
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: cannot read directory: {e}");
            }
        }
    }

    matches.sort_by_key(|p| {
        fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    matches.pop()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    agy_bridge::load_dotenv();
    let bridge = AgyBridge::builder().build()?;

    header("Phase 1: Generator — creating image");

    let gen_config = AgentConfig::builder()
        .system_instructions("You are an image generation assistant. When asked to generate an image, use the 'generate_image' tool. After the image is created, tell the user the image name and a one-line confirmation. Do not describe the image.".to_string())
        .capabilities(CapabilitiesConfig {
            enabled_tools: Some(vec![BuiltinTools::GenerateImage]),
            enable_subagents: false,
            ..CapabilitiesConfig::default()
        })
        .policies([PolicyRule::Allow("generate_image".to_string())])
        .build();

    let prompt = "Generate an image of a white and orange Birman cat sitting in front of a fish-shaped birthday cake with lit candles. Name it 'birman_birthday'.";
    println!(">>> {prompt}\n");

    let generator = bridge.agent(gen_config).await?;
    let text = generator.chat(prompt).await?.text().await?;
    println!("{text}");
    generator.shutdown().await?;

    header("Phase 2: Discriminator — describing image");

    let Some(image_path) = find_generated_image("birman_birthday") else {
        println!("ERROR: Could not find generated image on disk.");
        return Ok(());
    };

    println!("  Found image: {}", image_path.display());
    println!("  Size: {} bytes", fs::metadata(&image_path)?.len());

    let disc_config = AgentConfig::builder()
        .system_instructions("You are a visual analysis assistant. You will receive an image with no prior context. Describe exactly what you see: subject matter, colors, lighting, mood, and any notable details. Be specific and vivid.".to_string())
        .build();

    let image_bytes = fs::read(&image_path)?;
    let disc_prompt = Content::Multi {
        parts: vec![
            ContentPrimitive::Text {
                text: "What do you see in this image? Describe it in detail.".to_string(),
            },
            ContentPrimitive::Image(Image::png(image_bytes)),
        ],
    };

    println!(">>> Sending raw image bytes to fresh agent...\n");

    let discriminator = bridge.agent(disc_config).await?;
    let t2 = discriminator.chat(disc_prompt).await?.text().await?;
    println!("{t2}");

    discriminator.shutdown().await?;
    Ok(())
}
