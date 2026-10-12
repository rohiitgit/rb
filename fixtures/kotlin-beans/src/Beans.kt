package acme

fun dataBeans() = beans {
    bean<DbImpl>()
    bean("db") { Db() }
}

fun webBeans() = beans {
    bean("store") { Store(ref("db"), ref()) }
}
