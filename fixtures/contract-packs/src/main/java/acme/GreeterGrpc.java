package acme;

// Committed generated-style stub source: the build compiles without codegen.
public class GreeterGrpc {
  public static GreeterBlockingStub newBlockingStub(Object channel) {
    return new GreeterBlockingStub();
  }

  public static class GreeterBlockingStub {
    public String sayHello(Object request) {
      return "hi";
    }
  }
}
