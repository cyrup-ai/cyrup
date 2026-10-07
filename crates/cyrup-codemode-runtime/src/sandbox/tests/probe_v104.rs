//! TEMPORARY measurement probe for area 18 (pi v1.0.4 `b223082bb`). Not committed as is.
use serde_json::json;

use super::support::*;

#[tokio::test]
async fn probe_patches_to_builtins() {
    let sandbox = sandbox(vec![echo()]);
    let result = run(
        &sandbox,
        r#"
        Array.prototype.toJSON = () => null;
        Object.prototype.toJSON = () => 5;
        Promise.prototype.then = () => {};
        Map.prototype.get = () => undefined;
        globalThis.JSON = { stringify: () => "x", parse: () => "x" };
        store("k", [1]);
        return [await tools.echo([2]), JSON.stringify({ a: 1 })];
    "#,
    )
    .await;
    eprintln!("PROBE1 {result:#?}");
}

#[tokio::test]
async fn probe_frozen_intrinsics() {
    let sandbox = sandbox(vec![]);
    let result = run(
        &sandbox,
        r#"
        return [
            Object.getPrototypeOf(function* () {}).prototype,
            Object.getPrototypeOf(async function () {}),
            Object.getPrototypeOf(Int8Array).prototype,
            Object.getPrototypeOf([][Symbol.iterator]()),
            Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]())),
            Object.getPrototypeOf(new Map()[Symbol.iterator]()),
            Object.getPrototypeOf(/a/[Symbol.matchAll]("")),
        ].map((object) => Object.isFrozen(object));
    "#,
    )
    .await;
    eprintln!("PROBE2 {:?}", value(&result));
}

#[tokio::test]
async fn probe_instance_overrides() {
    let sandbox = sandbox(vec![]);
    let result = run(
        &sandbox,
        r#"
        const object = {};
        object.toString = () => "custom";
        function Legacy() {}
        Legacy.prototype = Object.create(Error.prototype);
        Legacy.prototype.constructor = Legacy;
        const bare = new Error();
        bare.message = "set later";
        class MyError extends Error {
            constructor(message) {
                super(message);
                this.name = "MyError";
            }
        }
        let patched = "silent";
        try { Error.prototype.name = "Patched"; } catch (error) { patched = error.constructor.name; }
        return [String(object), new Legacy().constructor === Legacy, bare.message, new MyError("x").name, Error.prototype.name, patched];
    "#,
    )
    .await;
    eprintln!("PROBE3 {result:#?}");
}

#[tokio::test]
async fn probe_non_string_error_fields() {
    let sandbox = sandbox(vec![]);
    let result = run(&sandbox, "const error = new Error('x'); error.message = 42; throw error;").await;
    eprintln!("PROBE4 {result:#?}");
    let result = run(&sandbox, "const e = new Error('x'); e.name = {}; throw e;").await;
    eprintln!("PROBE4b {result:#?}");
}

#[tokio::test]
async fn probe_serializer_patches() {
    let sandbox = sandbox(vec![echo()]);
    for code in [
        r#"Array.prototype.toJSON = () => null; store("k", [1]); return 1;"#,
        r#"Array.prototype.toJSON = () => null; return 1;"#,
        r#"Object.prototype.toJSON = () => 5; store("k", 1); return { a: 1 };"#,
        r#"Object.prototype.toJSON = () => 5; throw new Error("boom");"#,
        r#"Array.prototype[Symbol.iterator] = function* () {}; store("k", [1,2]); return 1;"#,
        r#"Map.prototype.get = () => undefined; Map.prototype.set = () => { throw 1 }; store("k", 1); return 1;"#,
        r#"Promise.prototype.then = () => {}; return await tools.echo(1);"#,
        r#"Object.defineProperty(Object.prototype, "message", { get() { throw new Error("no"); } }); throw new Error("x");"#,
    ] {
        let result = run(&sandbox, code).await;
        eprintln!("PROBE5 [{code}] => {result:?}");
    }
}
