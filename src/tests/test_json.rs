use std::path::Path;
use crate::analyze::json_parse;
pub struct TestJson {

}

impl TestJson {
    pub fn test_rwrw() {
        let path = Path::new("../tools_config.json");
        let js = json_parse::JsonParser::new(path.to_str().unwrap().to_string());
        let og_seed = js.get_seed();
        println!("Original seed: {:?}", &og_seed);
        let new_seed: u32 = if *og_seed.as_ref().unwrap().as_ref().unwrap() == 67 { 42 } else { 67 };
        println!("New seed: {:?}", new_seed);
        js.set_seed(&new_seed);
        println!("Seed updated successfully: {:?}", js.get_seed());
        js.set_seed(og_seed.as_ref().unwrap().as_ref().unwrap());
        println!("Seed restored successfully: {:?}", js.get_seed());
    }
}