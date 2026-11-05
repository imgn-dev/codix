CREATE SCHEMA billing;

CREATE TABLE billing.invoices (
    id BIGINT PRIMARY KEY,
    total NUMERIC(12, 2) NOT NULL
);

CREATE VIEW open_invoices AS SELECT * FROM billing.invoices WHERE total > 0;

CREATE SEQUENCE invoice_ids START 1;
