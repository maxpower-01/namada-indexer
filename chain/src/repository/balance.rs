use anyhow::Context;
use diesel::sql_types::BigInt;
use diesel::upsert::excluded;
use diesel::{
    sql_query, ExpressionMethods, PgConnection, QueryableByName, RunQueryDsl,
};
use orm::balances::BalancesInsertDb;
use orm::schema::balances;
use shared::balance::Balances;
pub const MAX_PARAM_SIZE: u16 = u16::MAX;

#[derive(QueryableByName)]
struct BalanceColCount {
    #[diesel(sql_type = BigInt)]
    count: i64,
}

pub fn insert_balance(
    transaction_conn: &mut PgConnection,
    balances: Balances,
) -> anyhow::Result<()> {
    diesel::insert_into(balances::table)
        .values::<&Vec<BalancesInsertDb>>(
            &balances
                .into_iter()
                .map(BalancesInsertDb::from_balance)
                .collect::<Vec<_>>(),
        )
        .on_conflict((balances::columns::owner, balances::columns::token))
        .do_update()
        .set(
            balances::columns::raw_amount
                .eq(excluded(balances::columns::raw_amount)),
        )
        .execute(transaction_conn)
        .context("Failed to update balances in db")?;

    anyhow::Ok(())
}

pub fn insert_balance_in_chunks(
    transaction_conn: &mut PgConnection,
    balances: Balances,
) -> anyhow::Result<()> {
    let balances_col_count = sql_query(
        "SELECT COUNT(*)
            FROM information_schema.columns
            WHERE table_schema = 'public'
            AND table_name = 'balances';",
    )
    .get_result::<BalanceColCount>(transaction_conn)?;

    for chunk in balances
        // We have to divide MAX_PARAM_SIZE by the number of columns in the
        // balances table to get the correct number of rows in the
        // chunk.
        .chunks((MAX_PARAM_SIZE as i64 / balances_col_count.count) as usize)
    {
        insert_balance(transaction_conn, chunk.to_vec())?
    }

    anyhow::Ok(())
}

#[cfg(test)]
mod tests {

    use anyhow::Context;
    use clap::Parser;
    use diesel::{BoolExpressionMethods, QueryDsl, SelectableHelper};
    use namada_sdk::token::Amount as NamadaAmount;
    use namada_sdk::uint::MAX_SIGNED_VALUE;
    use orm::balances::BalanceDb;
    use shared::balance::{Amount, Balance};
    use shared::id::Id;

    use super::*;
    use crate::config::TestConfig;
    use crate::test_db::TestDb;

    /// Test that the function correctly handles an empty `balances` input.
    #[tokio::test]
    async fn test_insert_balance_with_empty_balances_new() {
        let config = TestConfig::parse();
        let db = TestDb::new(&config);

        db.run_test(|conn| {
            insert_balance(conn, vec![])?;

            let queried_balance = query_all_balances(conn)?;

            assert_eq!(queried_balance.len(), 0,);

            anyhow::Ok(())
        })
        .await
        .expect("Failed to run test");
    }

    /// Test the basic functionality of inserting a single balance.
    #[tokio::test]
    async fn test_insert_balance_with_single_balance() {
        let config = TestConfig::parse();
        let db = TestDb::new(&config);

        db.run_test(|conn| {
            let owner = Id::Account(
                "tnam1qqshvryx9pngpk7mmzpzkjkm6klelgusuvmkc0uz".to_string(),
            );
            let token = Id::Account(
                "tnam1q87wtaqqtlwkw927gaff34hgda36huk0kgry692a".to_string(),
            );
            let amount = Amount::from(NamadaAmount::from_u64(100));

            let balance = Balance {
                owner: owner.clone(),
                token: token.clone(),
                amount: amount.clone(),
            };

            insert_balance(conn, vec![balance.clone()])?;

            let queried_balance = query_balance_by_address(conn, owner, token)?;

            assert_eq!(Amount::from(queried_balance.raw_amount), amount);

            anyhow::Ok(())
        })
        .await
        .expect("Failed to run test");
    }

    /// Test that the function updates existing balances when there is a
    /// conflict.
    #[tokio::test]
    async fn test_insert_balance_with_existing_balances_update() {
        let config = TestConfig::parse();
        let db = TestDb::new(&config);

        let owner = Id::Account(
            "tnam1qqshvryx9pngpk7mmzpzkjkm6klelgusuvmkc0uz".to_string(),
        );
        let token = Id::Account(
            "tnam1q87wtaqqtlwkw927gaff34hgda36huk0kgry692a".to_string(),
        );
        let amount = Amount::from(NamadaAmount::from_u64(100));

        let balance = Balance {
            owner: owner.clone(),
            token: token.clone(),
            amount: amount.clone(),
        };

        db.run_test(move |conn| {
            seed_balance(conn, vec![balance.clone()])?;

            let new_amount = Amount::from(NamadaAmount::from_u64(200));
            let new_balance = Balance {
                amount: new_amount.clone(),
                ..(balance.clone())
            };

            insert_balance(conn, vec![new_balance])?;

            let queried_balance =
                query_balance_by_address(conn, owner.clone(), token.clone())?;

            assert_eq!(Amount::from(queried_balance.raw_amount), new_amount);

            anyhow::Ok(())
        })
        .await
        .expect("Failed to run test");
    }

    /// Test the function's behavior when inserting balances that cause a
    /// conflict.
    #[tokio::test]
    async fn test_insert_balance_with_conflicting_owners() {
        let config = TestConfig::parse();
        let db = TestDb::new(&config);

        let owner = Id::Account(
            "tnam1qqshvryx9pngpk7mmzpzkjkm6klelgusuvmkc0uz".to_string(),
        );
        let token = Id::Account(
            "tnam1qxfj3sf6a0meahdu9t6znp05g8zx4dkjtgyn9gfu".to_string(),
        );
        let amount = Amount::from(NamadaAmount::from_u64(100));

        let balance = Balance {
            owner: owner.clone(),
            token: token.clone(),
            amount: amount.clone(),
        };

        db.run_test(move |conn| {
            seed_balance(conn, vec![balance.clone()])?;

            let new_amount = Amount::from(NamadaAmount::from_u64(200));
            let new_token = Id::Account(
                "tnam1q87wtaqqtlwkw927gaff34hgda36huk0kgry692a".to_string(),
            );
            let new_balance = Balance {
                token: new_token.clone(),
                amount: new_amount.clone(),
                ..(balance.clone())
            };

            insert_balance(conn, vec![new_balance])?;

            let queried_balance =
                query_balance_by_address(conn, owner.clone(), token.clone())?;

            let queried_balance_new = query_balance_by_address(
                conn,
                owner.clone(),
                new_token.clone(),
            )?;

            assert_eq!(Amount::from(queried_balance.raw_amount), amount);
            assert_eq!(
                Amount::from(queried_balance_new.raw_amount),
                new_amount
            );

            anyhow::Ok(())
        })
        .await
        .expect("Failed to run test");
    }
    /// Test the function's behavior when inserting balances that cause a
    /// conflict.
    #[tokio::test]
    async fn test_insert_balance_with_conflicting_tokens() {
        let config = TestConfig::parse();
        let db = TestDb::new(&config);

        let owner = Id::Account(
            "tnam1qqshvryx9pngpk7mmzpzkjkm6klelgusuvmkc0uz".to_string(),
        );
        let token = Id::Account(
            "tnam1qxfj3sf6a0meahdu9t6znp05g8zx4dkjtgyn9gfu".to_string(),
        );
        let amount = Amount::from(NamadaAmount::from_u64(100));

        let balance = Balance {
            owner: owner.clone(),
            token: token.clone(),
            amount: amount.clone(),
        };

        db.run_test(move |conn| {
            seed_balance(conn, vec![balance.clone()])?;

            let new_owner = Id::Account(
                "tnam1q9rhgyv3ydq0zu3whnftvllqnvhvhm270qxay5tn".to_string(),
            );
            let new_amount = Amount::from(NamadaAmount::from_u64(200));
            let new_balance = Balance {
                amount: new_amount.clone(),
                owner: new_owner.clone(),
                ..(balance.clone())
            };

            insert_balance(conn, vec![new_balance])?;

            let queried_balance =
                query_balance_by_address(conn, owner.clone(), token.clone())?;

            let queried_balance_new = query_balance_by_address(
                conn,
                new_owner.clone(),
                token.clone(),
            )?;

            assert_eq!(Amount::from(queried_balance.raw_amount), amount);
            assert_eq!(
                Amount::from(queried_balance_new.raw_amount),
                new_amount
            );

            anyhow::Ok(())
        })
        .await
        .expect("Failed to run test");
    }

    /// Test the function's ability to handle a large number of balance inserts
    /// efficiently.
    #[tokio::test]
    async fn test_insert_balance_with_large_number_of_balances() {
        let config = TestConfig::parse();
        let db = TestDb::new(&config);

        db.run_test(move |conn| {
            let fake_balances =
                (0..10000).map(|_| Balance::fake()).collect::<Vec<_>>();

            insert_balance(conn, fake_balances.clone())?;

            assert_eq!(query_all_balances(conn)?.len(), fake_balances.len());

            anyhow::Ok(())
        })
        .await
        .expect("Failed to run test");
    }

    /// Test how the function handles extremely large balance values.
    #[tokio::test]
    async fn test_insert_balance_with_extremely_large_balance_value() {
        let config = TestConfig::parse();
        let db = TestDb::new(&config);

        db.run_test(|conn| {
            let owner = Id::Account(
                "tnam1qqshvryx9pngpk7mmzpzkjkm6klelgusuvmkc0uz".to_string(),
            );
            let token = Id::Account(
                "tnam1q87wtaqqtlwkw927gaff34hgda36huk0kgry692a".to_string(),
            );
            let max_amount = Amount::from(NamadaAmount::from(MAX_SIGNED_VALUE));

            let balance = Balance {
                owner: owner.clone(),
                token: token.clone(),
                amount: max_amount.clone(),
            };

            insert_balance(conn, vec![balance.clone()])?;

            let queried_balance = query_balance_by_address(conn, owner, token)?;

            assert_eq!(Amount::from(queried_balance.raw_amount), max_amount);

            anyhow::Ok(())
        })
        .await
        .expect("Failed to run test");
    }

    /// Test that we can insert more than u16::MAX balances
    #[tokio::test]
    async fn test_insert_balance_in_chunks_with_max_param_size_plus_one() {
        let config = TestConfig::parse();
        let db = TestDb::new(&config);

        db.run_test(|conn| {
            let mps = MAX_PARAM_SIZE as u32;

            let balances =
                (0..mps + 1).map(|_| Balance::fake()).collect::<Vec<_>>();

            let res = insert_balance_in_chunks(conn, balances)?;

            assert_eq!(res, ());

            anyhow::Ok(())
        })
        .await
        .expect("Failed to run test");
    }

    /// Test that we can insert less than u16::MAX balances using chunks
    #[tokio::test]
    async fn test_insert_balance_in_chunks_with_1000_params() {
        let config = TestConfig::parse();
        let db = TestDb::new(&config);

        db.run_test(|conn| {
            let balances =
                (0..1000).map(|_| Balance::fake()).collect::<Vec<_>>();

            let res = insert_balance_in_chunks(conn, balances)?;

            assert_eq!(res, ());

            anyhow::Ok(())
        })
        .await
        .expect("Failed to run test");
    }

    fn seed_balance(
        conn: &mut PgConnection,
        balances: Vec<Balance>,
    ) -> anyhow::Result<()> {
        diesel::insert_into(balances::table)
            .values::<&Vec<BalancesInsertDb>>(
                &balances
                    .into_iter()
                    .map(BalancesInsertDb::from_balance)
                    .collect::<Vec<_>>(),
            )
            .execute(conn)
            .context("Failed to update balances in db")?;

        anyhow::Ok(())
    }

    fn query_balance_by_address(
        conn: &mut PgConnection,
        owner: Id,
        token: Id,
    ) -> anyhow::Result<BalanceDb> {
        balances::table
            .filter(
                balances::dsl::owner
                    .eq(owner.to_string())
                    .and(balances::dsl::token.eq(token.to_string())),
            )
            .select(BalanceDb::as_select())
            .first(conn)
            .context("Failed to query balance by address")
    }

    fn query_all_balances(
        conn: &mut PgConnection,
    ) -> anyhow::Result<Vec<BalanceDb>> {
        balances::table
            .select(BalanceDb::as_select())
            .get_results(conn)
            .context("Failed to query balance by address")
    }
}
