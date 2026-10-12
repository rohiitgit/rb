package acme

interface GreeterApi {
    fun greet(name: String): String
}

class Greeter : GreeterApi {
    override fun greet(name: String): String {
        return shout(name)
    }
}

fun shout(s: String): String {
    return s
}
