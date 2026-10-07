package com.acme.lombok;

import lombok.Builder;
import lombok.Value;
import lombok.With;

@Value
@With
@Builder
public class Money {
    long amount;
    String currency;

    public Money plus(Money other) {
        return withAmount(getAmount() + other.getAmount());
    }
}
