use crate::models::{Cat, CreateCatRequest};

pub struct CatService;

impl CatService {
    pub fn get_all_cats() -> Vec<Cat> {
        vec![
            Cat {
                id: 1,
                name: "Felix".to_string(),
                age: 2,
            },
            Cat {
                id: 2,
                name: "Garfield".to_string(),
                age: 3,
            },
            Cat {
                id: 3,
                name: "Whiskers".to_string(),
                age: 1,
            },
        ]
    }

    pub fn find_cat_by_id(id: u32) -> Option<Cat> {
        Self::get_all_cats().into_iter().find(|cat| cat.id == id)
    }

    pub fn create_cat(cat_req: CreateCatRequest) -> Cat {
        Cat::from(cat_req)
    }

    pub fn update_cat(id: u32, cat_req: CreateCatRequest) -> Option<Cat> {
        if Self::find_cat_by_id(id).is_some() {
            let updated_cat: Cat = cat_req.into();
            Some(Cat {
                id,
                name: updated_cat.name,
                age: updated_cat.age,
            })
        } else {
            None
        }
    }

    pub fn delete_cat(id: u32) -> bool {
        Self::find_cat_by_id(id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_all_cats() {
        let cats = CatService::get_all_cats();
        assert!(!cats.is_empty());
        assert_eq!(cats.len(), 3); // Based on your hardcoded data
    }

    #[test]
    fn test_find_cat_by_id() {
        let cat = CatService::find_cat_by_id(1);
        assert!(cat.is_some());
        assert_eq!(cat.unwrap().name, "Felix");

        let non_existent = CatService::find_cat_by_id(999);
        assert!(non_existent.is_none());
    }

    #[test]
    fn test_create_cat() {
        let request = CreateCatRequest {
            name: "TestCat".to_string(),
            age: 5,
        };

        let cat = CatService::create_cat(request);
        assert_eq!(cat.name, "TestCat");
        assert_eq!(cat.age, 5);
    }

    #[test]
    fn test_update_cat() {
        let request = CreateCatRequest {
            name: "UpdatedCat".to_string(),
            age: 4,
        };

        let updated = CatService::update_cat(1, request);
        assert!(updated.is_some());
        assert_eq!(updated.unwrap().name, "UpdatedCat");

        let non_existent = CatService::update_cat(
            999,
            CreateCatRequest {
                name: "Test".to_string(),
                age: 1,
            },
        );
        assert!(non_existent.is_none());
    }

    #[test]
    fn test_delete_cat() {
        assert!(CatService::delete_cat(1));
        assert!(!CatService::delete_cat(999));
    }
}
