package com.acme.lombok;

public class Use {
    public String run() {
        User user = new User("ada", true);
        Money total = Money.builder().amount(1).currency("EUR").build().plus(new Money(2, "EUR"));
        Account account = new Account();
        account.setId(4);
        return user.greet() + total + account.twice() + new Plain().calc() + new Shadow().run()
            + new Marked().use();
    }
}
