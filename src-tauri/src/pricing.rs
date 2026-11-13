//! Rust port of `src/lib/money.ts` (order totals and VAT). The Host uses it so stored totals
//! never depend on what a client sends. Both implementations are checked against
//! `test-vectors/pricing.json`, so they agree to the centime.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Tax {
    pub id: i64,
    pub rate_bp: i64,
    pub price_include: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Discount {
    Percent { bp: i64 },
    Fixed { cents: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LineInput {
    pub unit_cents: i64,
    pub qty: i64,
    pub tax: Tax,
    #[serde(default)]
    pub discount: Option<Discount>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct LineTotals {
    pub gross: i64,
    pub discount: i64,
    pub base: i64,
    pub tax: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TaxBucket {
    pub tax_id: i64,
    pub rate_bp: i64,
    pub base: i64,
    pub tax: i64,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OrderTotals {
    pub lines: Vec<LineTotals>,
    pub subtotal: i64,
    pub order_discount: i64,
    pub base: i64,
    pub tax: i64,
    pub total: i64,
    pub vat: Vec<TaxBucket>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Rounding {
    #[default]
    PerLine,
    Global,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ExtraFee {
    pub cents: i64,
    pub tax: Tax,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OrderOpts {
    #[serde(default)]
    pub rounding: Rounding,
    #[serde(default)]
    pub order_discount: Option<Discount>,
    #[serde(default)]
    pub extra_fee: Option<ExtraFee>,
}

#[derive(Debug, PartialEq)]
pub struct PricingError(pub &'static str);

/// Integer division rounded half away from zero.
pub fn div_round(n: i64, d: i64) -> i64 {
    assert!(d != 0, "division by zero");
    let sign = n.signum() * d.signum();
    let (n, d) = (n.abs(), d.abs());
    let q = n / d;
    let r = n - q * d;
    sign * if 2 * r >= d { q + 1 } else { q }
}

pub fn percent_of(amount: i64, bp: i64) -> i64 {
    div_round(amount * bp, 10000)
}

pub fn split_included(total: i64, rate_bp: i64) -> (i64, i64) {
    let base = div_round(total * 10000, 10000 + rate_bp);
    (base, total - base)
}

pub fn discount_amount(gross: i64, d: Option<Discount>) -> Result<i64, PricingError> {
    match d {
        None => Ok(0),
        Some(Discount::Percent { bp }) => {
            if !(0..=10000).contains(&bp) {
                return Err(PricingError("discount percent out of range"));
            }
            Ok(percent_of(gross, bp))
        }
        Some(Discount::Fixed { cents }) => {
            if cents < 0 {
                return Err(PricingError("negative discount"));
            }
            Ok(cents.min(gross))
        }
    }
}

pub fn compute_line(l: &LineInput) -> Result<LineTotals, PricingError> {
    if l.qty < 0 {
        return Err(PricingError("qty must be >= 0"));
    }
    let gross = l.unit_cents * l.qty;
    let discount = discount_amount(gross, l.discount)?;
    let net = gross - discount;
    if l.tax.price_include {
        let (base, tax) = split_included(net, l.tax.rate_bp);
        Ok(LineTotals { gross, discount, base, tax, total: net })
    } else {
        let tax = percent_of(net, l.tax.rate_bp);
        Ok(LineTotals { gross, discount, base: net, tax, total: net + tax })
    }
}

/// Largest-remainder split; parts always sum to `amount`.
pub fn allocate(amount: i64, weights: &[i64]) -> Vec<i64> {
    if weights.is_empty() {
        return vec![];
    }
    let sum: i64 = weights.iter().sum();
    if sum == 0 {
        return weights.iter().enumerate().map(|(i, _)| if i == 0 { amount } else { 0 }).collect();
    }
    let sign = amount.signum();
    let abs = amount.abs();
    let mut parts: Vec<i64> = weights.iter().map(|w| (abs * w).div_euclid(sum)).collect();
    let rems: Vec<i64> = weights.iter().zip(&parts).map(|(w, p)| abs * w - p * sum).collect();
    let mut rest = abs - parts.iter().sum::<i64>();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|&a, &b| rems[b].cmp(&rems[a]).then(a.cmp(&b)));
    let mut k = 0;
    while rest > 0 {
        parts[order[k % order.len()]] += 1;
        k += 1;
        rest -= 1;
    }
    parts.into_iter().map(|p| p * sign).collect()
}

pub fn compute_order(lines: &[LineInput], opts: &OrderOpts) -> Result<OrderTotals, PricingError> {
    let first: Vec<LineTotals> = lines.iter().map(compute_line).collect::<Result<_, _>>()?;
    let subtotal: i64 = first.iter().map(|l| l.total).sum();
    let order_discount = discount_amount(subtotal, opts.order_discount)?;
    let shares = allocate(order_discount, &first.iter().map(|l| l.total).collect::<Vec<_>>());

    let mut nets: Vec<(Tax, i64)> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if l.tax.price_include {
                (l.tax, first[i].total - shares[i])
            } else {
                let share_ht = div_round(shares[i] * 10000, 10000 + l.tax.rate_bp);
                (l.tax, first[i].base - share_ht)
            }
        })
        .collect();
    if let Some(fee) = opts.extra_fee {
        if fee.cents > 0 {
            nets.push((fee.tax, fee.cents));
        }
    }

    let split = |tax: Tax, net: i64| -> (i64, i64, i64) {
        if tax.price_include {
            let (b, t) = split_included(net, tax.rate_bp);
            (b, t, net)
        } else {
            let t = percent_of(net, tax.rate_bp);
            (net, t, net + t)
        }
    };

    let final_lines: Vec<LineTotals> = (0..lines.len())
        .map(|i| {
            let (tax, net) = nets[i];
            let (base, t, total) = split(tax, net);
            LineTotals { gross: first[i].gross, discount: first[i].discount + (first[i].total - total), base, tax: t, total }
        })
        .collect();

    // Buckets by tax id, in first-seen order (like a JS Map).
    struct B {
        tax: Tax,
        nets: i64,
        line_base: i64,
        line_tax: i64,
    }
    let mut buckets: Vec<B> = Vec::new();
    for (i, (tax, net)) in nets.iter().enumerate() {
        let (lb, lt) = if i < final_lines.len() {
            (final_lines[i].base, final_lines[i].tax)
        } else {
            let (b, t, _) = split(*tax, *net);
            (b, t)
        };
        match buckets.iter_mut().find(|b| b.tax.id == tax.id) {
            Some(b) => {
                b.nets += net;
                b.line_base += lb;
                b.line_tax += lt;
            }
            None => buckets.push(B { tax: *tax, nets: *net, line_base: lb, line_tax: lt }),
        }
    }
    let mut vat: Vec<TaxBucket> = buckets
        .iter()
        .map(|b| {
            let (base, tax, total) = match opts.rounding {
                Rounding::PerLine => (b.line_base, b.line_tax, b.line_base + b.line_tax),
                Rounding::Global => split(b.tax, b.nets),
            };
            TaxBucket { tax_id: b.tax.id, rate_bp: b.tax.rate_bp, base, tax, total }
        })
        .collect();
    vat.sort_by(|a, b| a.rate_bp.cmp(&b.rate_bp).then(a.tax_id.cmp(&b.tax_id)));
    let base: i64 = vat.iter().map(|b| b.base).sum();
    let tax: i64 = vat.iter().map(|b| b.tax).sum();
    Ok(OrderTotals { lines: final_lines, subtotal, order_discount, base, tax, total: base + tax, vat })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Case {
        name: String,
        lines: Vec<LineInput>,
        opts: OrderOpts,
        expected: OrderTotals,
    }

    #[test]
    fn matches_shared_vectors() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("../../test-vectors/pricing.json")).unwrap();
        assert!(cases.len() >= 300);
        for c in &cases {
            let got = compute_order(&c.lines, &c.opts).unwrap_or_else(|e| panic!("{}: {:?}", c.name, e));
            assert_eq!(got, c.expected, "case {}", c.name);
        }
    }

    #[test]
    fn rounding_basics() {
        assert_eq!(div_round(5, 2), 3);
        assert_eq!(div_round(-5, 2), -3);
        assert_eq!(div_round(4, 3), 1);
        assert_eq!(split_included(4500, 1000), (4091, 409));
        assert_eq!(allocate(100, &[1, 1, 1]), vec![34, 33, 33]);
        assert_eq!(allocate(-100, &[1, 1, 1]), vec![-34, -33, -33]);
        assert!(discount_amount(100, Some(Discount::Percent { bp: 10001 })).is_err());
    }
}
