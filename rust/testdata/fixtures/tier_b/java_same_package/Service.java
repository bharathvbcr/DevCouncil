package com.acme.billing;

class Service {
    int compute(int amount) {
        return amount + fee();
    }

    int fee() {
        return 1;
    }

    private int retired(int amount) {
        return amount * 2;
    }
}
