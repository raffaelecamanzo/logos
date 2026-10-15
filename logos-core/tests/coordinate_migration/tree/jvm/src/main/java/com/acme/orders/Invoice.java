package com.acme.orders;

/** An invoice for one order. */
public class Invoice {
    private final int amount;

    public Invoice(int amount) {
        this.amount = amount;
    }

    public int amount() {
        return Math.max(amount, 0);
    }
}
