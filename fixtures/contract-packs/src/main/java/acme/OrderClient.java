package acme;

public class OrderClient {
  private final UsersApi usersApi;

  public OrderClient(UsersApi usersApi) {
    this.usersApi = usersApi;
  }

  public String names() {
    return String.join(",", usersApi.listUsers());
  }

  public String greet(Object channel, Object request) {
    return GreeterGrpc.newBlockingStub(channel).sayHello(request);
  }
}
