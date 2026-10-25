use std::path::Path;
use crate::analyze::json_parse;
pub struct TestJson {

}

impl TestJson {
    pub fn test_rwrw() {
        let path = Path::new("../tools_config.json");
        println!("Full path: {:?}", path.canonicalize().unwrap());
        let js = json_parse::JsonParser::new(path.to_str().unwrap().to_string());
        let og_seed = js.get_seed().unwrap().expect("no seed in config");
        println!("Original seed: {:?}", og_seed);
        let new_seed: u32 = if og_seed == 67 { 42 } else { 67 };
        println!("New seed: {:?}", new_seed);
        js.set_seed(&new_seed).unwrap();

        match js.get_seed() {
            Ok(Some(seed)) if seed == new_seed => {
                println!("Seed updated successfully: {:?}", seed);
            }
            output => panic!("Unexpected seed after update: {:?}", output),
        }

        js.set_seed(&og_seed).unwrap();
        match js.get_seed() {
            Ok(Some(seed)) if seed == og_seed => {
                println!("Seed restored successfully: {:?}", seed);
            }
            output => panic!("Unexpected seed after restore: {:?}", output),
        }

        println!("Test passed!");
    }
}