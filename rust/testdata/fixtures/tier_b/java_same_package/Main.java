package com.acme.billing;

public class Main {
    public static void main(String[] args) {
        Service service = new Service();
        System.out.println(service.compute(41));
        Ledger ledger = new Ledger();
        System.out.println(ledger.compute(2));
    }
}
