use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufRead;

pub type PinyinComponents = (String, String);
pub type PinyinTable = HashMap<char, PinyinComponents>;

pub fn load_pinyin_table(path: &str) -> PinyinTable {
  let file = File::open(path).expect("Failed to open pinyin table");
  let reader = std::io::BufReader::new(file);
  let mut raw_map: HashMap<char, Vec<(String, u64)>> = HashMap::new();
  for line in reader.lines() {
    let line = line.expect("Read line");
    let parts: Vec<&str> = line.split(',').collect();
    if parts.len() < 3 {
      continue;
    }
    let c_str = parts[0].trim();
    let py = parts[1].trim().to_string();
    let freq: u64 = parts[2].trim().parse().unwrap_or(0);
    if let Some(c) = c_str.chars().next() {
      raw_map.entry(c).or_default().push((py, freq));
    }
  }
  let mut table = HashMap::new();
  for (c, mut list) in raw_map {
    // 按频率降序，频率相同按拼音字典序
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    if let Some((best_py, _)) = list.first() {
      if let Some(comps) = split_pinyin(best_py) {
        table.insert(c, comps);
      }
    }
  }
  table
}

// 拆分逻辑：最长的不包含 aeiouv 的前缀为声母，其余为韵母
// 特殊情况：无韵母的（如 hm, hng 等纯辅音），忽略
fn split_pinyin(py: &str) -> Option<PinyinComponents> {
  let vowels = ['a', 'e', 'i', 'o', 'u', 'v']; // v represents ü

  // 检查是否全无元音 (例如 hm, ng, m)
  let has_vowel = py.chars().any(|c| vowels.contains(&c));
  if !has_vowel {
    return None;
  }

  let mut split_idx = py.len();
  for (i, ch) in py.char_indices() {
    if vowels.contains(&ch) {
      split_idx = i;
      break;
    }
  }

  // 整个都是声母？不可能，因为上面检查了有元音
  let (init, fin) = py.split_at(split_idx);

  Some((init.to_string(), fin.to_string()))
}

pub fn get_text_components(text: &str, table: &PinyinTable) -> (HashSet<String>, HashSet<String>) {
  let mut inits = HashSet::new();
  let mut finals = HashSet::new();
  for c in text.chars() {
    if let Some((i, f)) = table.get(&c) {
      inits.insert(i.clone());
      finals.insert(f.clone());
    }
  }
  (inits, finals)
}

pub fn validate_char(
  c: char,
  table: &PinyinTable,
  banned_inits: &HashSet<String>,
  banned_finals: &HashSet<String>,
) -> Result<(), String> {
  // 如果不在表中，即为非法
  if !table.contains_key(&c) {
    return Err(format!("Char '{}' invalid (not in table).", c));
  }
  let (i, f) = &table[&c];
  if banned_inits.contains(i) {
    return Err(format!("Char '{}' uses banned initial '{}'.", c, i));
  }
  if banned_finals.contains(f) {
    return Err(format!("Char '{}' uses banned final '{}'.", c, f));
  }
  Ok(())
}
