package acme

class VDb {
    fun vload(): String = "rows"
}

class VSvc(db: VDb) {
    fun vstock(): String = "stock"
}

// Contract M6k S2: the top-level val form. No enclosing definition exists, so the
// engine anchors each registration at its bound type (`VDb`, `VSvc`); the `ref()`
// wires through its qualifier with evidence in this file.
val appBeans = beans {
    bean("vdb") { VDb() }
    bean("vstore") { VSvc(ref("vdb")) }
}
