//! Diesel table definitions, matching `migrations/.../up.sql`.

diesel::table! {
    extractions (name, version) {
        name -> Text,
        version -> Text,
        downloads -> BigInt,
        status -> Text,
        ir_path -> Nullable<Text>,
        n_items -> Nullable<Integer>,
        n_modules -> Nullable<Integer>,
        n_edges -> Nullable<Integer>,
        elapsed_sec -> Nullable<Double>,
        error -> Nullable<Text>,
        ts -> BigInt,
    }
}

diesel::table! {
    analyses (name, version) {
        name -> Text,
        version -> Text,
        status -> Text,
        headline -> Nullable<Double>,
        headline_depth_averaged -> Nullable<Double>,
        modularity_term -> Nullable<Double>,
        divergence_term -> Nullable<Double>,
        acyclicity_term -> Nullable<Double>,
        encapsulation_term -> Nullable<Double>,
        is_acyclic -> Nullable<Integer>,
        over_exposed_fraction -> Nullable<Double>,
        mean_leak_cost -> Nullable<Double>,
        n_real_items -> Nullable<Integer>,
        n_module_nodes -> Nullable<Integer>,
        anomaly -> Nullable<Text>,
        elapsed_ms -> Nullable<Double>,
        error -> Nullable<Text>,
        ts -> BigInt,
    }
}
