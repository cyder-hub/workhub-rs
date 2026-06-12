pub const MAX_SAME_ORIGIN_REDIRECTS: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_contract_exposes_redirect_default() {
        assert_eq!(MAX_SAME_ORIGIN_REDIRECTS, 3);
    }
}
