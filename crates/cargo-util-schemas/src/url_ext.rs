use url::Url;

pub trait UrlExt {
    /// Removes the specified query parameters from the URL.
    fn remove_query_params(&mut self, keys: &[&str]);
}

impl UrlExt for Url {
    fn remove_query_params(&mut self, keys: &[&str]) {
        let query: String = self
            .query_pairs()
            .filter(|(k, _)| !keys.iter().any(|&key| key == k.as_ref()))
            .fold(
                url::form_urlencoded::Serializer::new(String::new()),
                |mut ser, (k, v)| {
                    ser.append_pair(&k, &v);
                    ser
                },
            )
            .finish();
        if query.is_empty() {
            self.set_query(None);
        } else {
            self.set_query(Some(&query));
        }
    }
}
