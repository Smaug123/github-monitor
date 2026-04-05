fn main() {
    println!("Hello, world!");
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_placeholder(x in 0i64..1000) {
            prop_assert!(x >= 0);
        }
    }
}
