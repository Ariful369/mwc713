use grin_util::secp::pedersen;
use grin_api::Output;
use uuid::Uuid;
use grin_keychain::Identifier;
use std::collections::HashMap;
use common::config::{Wallet713Config, WalletConfig};
use common::{ErrorKind, Result};

use super::api::{controller, display};
use super::backend::Backend;
use super::types::{
    Arc, BlockFees, CbData, ExtKeychain, HTTPNodeClient, Mutex, OutputData, NodeClient, SecretKey,
    Slate, Transaction, TxLogEntry, WalletBackend, WalletInfo, WalletInst, WalletSeed,
};

use crate::common::crypto::Hex;
use crate::common::hasher::derive_address_key;
use crate::contacts::AddressBook;
use crate::wallet::api::Wallet713OwnerAPI;
use crate::wallet::types::TxProof;

pub struct Wallet {
    pub active_account: String,
    backend: Option<Arc<Mutex<Backend<HTTPNodeClient, ExtKeychain>>>>,
    max_auto_accept_invoice: Option<u64>,
}

impl Wallet {
    pub fn new(max_auto_accept_invoice: Option<u64>) -> Self {
        Self {
            active_account: "default".to_string(),
            backend: None,
            max_auto_accept_invoice,
        }
    }

    pub fn seed_exists(config: &Wallet713Config) -> bool {
        let wallet_config = config.as_wallet_config().unwrap();
        WalletSeed::seed_file_exists(&wallet_config.data_file_dir).is_err()
    }

    pub fn unlock(
        &mut self,
        config: &Wallet713Config,
        account: &str,
        passphrase: &str,
    ) -> Result<()> {
        self.lock();
        self.create_wallet_instance(config, account, passphrase)
            .map_err(|_| ErrorKind::WalletUnlockFailed)?;
        self.active_account = account.to_string();
        Ok(())
    }

    pub fn getnextkey(
        &mut self,
        amount: u64,
    ) -> Result<()> {
        let wallet = self.get_wallet_instance()?;

        controller::owner_single_use(wallet.clone(), |api| {
            let key = api.getnextkey(amount)?;
            println!("{:?}", key);
            Ok(())
        })?;
        Ok(())
    }

    pub fn node_info(
        &mut self) -> Result<()> {
        // get wallet instance
        let wallet = self.get_wallet_instance()?;

        // create a single use wallet api
        controller::owner_single_use(wallet.clone(), |api| {
            let ni = api.node_info().unwrap();
            // this is an error condition
            if ni.height == 0 && ni.total_difficulty == 0 {
                cli_message!("Error occured trying to contact node!");
            }
            else
            {
                // otherwise it worked, print it out here.
                cli_message!("Node Info:");
                cli_message!("Height: {}", ni.height);
                cli_message!("Total_Difficulty: {}", ni.total_difficulty);
                cli_message!("PeerInfo: {:?}", ni.peers);
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn account_exists(
        &mut self,
        account: &str) -> Result<(bool)> {
        let mut ret = false;
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| {
            let acct_mappings = api.accounts()?;
            for m in acct_mappings {
                if m.label == account {
                    ret = true;
                }
            }
            Ok(())
        })?;
        Ok(ret)
    }
    

    pub fn show_mnemonic(&self, config: &Wallet713Config, passphrase: &str) -> Result<()> {
        let wallet_config = config.as_wallet_config()?;
        let seed = WalletSeed::from_file(&wallet_config.data_file_dir, passphrase)?;
        seed.show_recovery_phrase()?;
        Ok(())
    }

    pub fn lock(&mut self) {
        self.backend = None;
    }

    pub fn is_locked(&self) -> bool {
        self.backend.is_none()
    }

    pub fn complete(
        &mut self,
        seed: WalletSeed,
        config: &Wallet713Config,
        account: &str,
        passphrase: &str,
        create_new: bool,
    ) -> Result<(WalletSeed)> {
        let wallet_config = config.as_wallet_config()?;
        let seed = self.init_seed(&wallet_config, passphrase, create_new, true, Some(seed))?;
        self.init_backend(&wallet_config, &config, passphrase)?;
        self.unlock(config, account, passphrase)?;
        Ok(seed)
    }

    pub fn init(
        &mut self,
        config: &Wallet713Config,
        passphrase: &str,
        create_new: bool,
    ) -> Result<(WalletSeed)> {
        let wallet_config = config.as_wallet_config()?;
        let seed = self.init_seed(&wallet_config, passphrase, create_new, false, None)?;
        Ok(seed)
    }

    pub fn restore_seed(
        &self,
        config: &Wallet713Config,
        words: &Vec<&str>,
        passphrase: &str,
    ) -> Result<()> {
        let wallet_config = config.as_wallet_config()?;
        WalletSeed::recover_from_phrase(&wallet_config.data_file_dir, &words.join(" "), passphrase)?;
        Ok(())
    }

    pub fn list_accounts(&self) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| {
            let acct_mappings = api.accounts()?;
            display::accounts(acct_mappings);
            Ok(())
        })?;
        Ok(())
    }

    pub fn rename_account(&self, old_name: &str, new_name: &str) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| {
            api.rename_account_path(old_name, new_name)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn create_account(&self, name: &str) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| {
            api.create_account_path(name)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn info(&self, refresh: bool, confirmations: u64) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| {
            let (mut validated, wallet_info) = api.retrieve_summary_info(refresh, confirmations, None, None)?;
            if !refresh  { validated = true; }
            display::info(&self.active_account, &wallet_info, validated, true);
            Ok(())
        })?;
        Ok(())
    }

    pub fn get_id(&self, slate_id: Uuid) -> Result<(u32)> {
        let mut id = 1;
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| {
            let (_height, _) = api.node_height()?;
            id = api.retrieve_tx_id_by_slate_id(slate_id)?;
            Ok(())
        })?;

        Ok(id)
    }

    pub fn txs_count(&self) -> Result<(usize)> {
        let wallet = self.get_wallet_instance()?;
        let mut txs_count = 0;
        controller::owner_single_use(wallet.clone(), |api| {
            let (_, txs) = api.retrieve_txs_with_proof_flag(true, None, None, 0, 0)?;
            txs_count = txs.len();
            Ok(())
        })?;
        Ok(txs_count)
    }

    pub fn txs(&self, address_book: Option<Arc<Mutex<AddressBook>>>, pagination_start: u32, pagination_length: u32) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| {
            let (height, _) = api.node_height()?;
            let (validated, txs) = api.retrieve_txs_with_proof_flag(true, None, None, pagination_start, pagination_length)?;
            display::txs(
                &self.active_account,
                height,
                validated,
                txs,
                true,
                true,
                address_book,
            )?;
            Ok(())
        })?;

        Ok(())
    }

    pub fn total_value(&self, minimum_confirmations: u64, output_list: Option<Vec<&str>>) -> Result<(u64)> {
        let wallet = self.get_wallet_instance()?;

        let mut value = 0;
        let _result = controller::owner_single_use(wallet.clone(), |api| {
            let (height, _) = api.node_height()?;
            let (_validated, outputs) = api.retrieve_outputs(false, true, None, 0, 0)?;

            for o in outputs {
                let mut found: bool = false;
                if output_list.is_some() {
                    let ol = output_list.clone().unwrap();
                    for lo in ol {
                        if o.0.commit.is_some() && lo == o.0.commit.clone().unwrap() {
                            found = true;
                            break;
                        }
                    }
                }
                else
                {
                    found = true;
                }

                if found && o.0.eligible_to_spend(height, minimum_confirmations) {
                    value += o.0.value;
                }
            }
            Ok(())
        })?;
        Ok(value)
    }

    pub fn all_output_count(&self, show_spent: bool) -> Result<(usize)> {
        let wallet = self.get_wallet_instance()?;
        let mut count = 0;
        controller::owner_single_use(wallet.clone(), |api| {
            let (_, outputs) = api.retrieve_outputs(show_spent, true, None, 0, 0)?;
            count = outputs.len();
            Ok(())
        })?;
        Ok(count)
    }

    pub fn output_count(&self, minimum_confirmations: u64, output_list: Option<Vec<&str>>) -> Result<(usize)> {
        let wallet = self.get_wallet_instance()?;

        let mut count = 0;
        let _result = controller::owner_single_use(wallet.clone(), |api| {
            let (height, _) = api.node_height()?;
            let (_validated, outputs) = api.retrieve_outputs(false, true, None, 0, 0)?;

            for o in outputs {
                let mut found: bool = false;
                if output_list.is_some() {
                    let ol = output_list.clone().unwrap();
                    for lo in ol {
                        if o.0.commit.is_some() && lo == o.0.commit.clone().unwrap() {
                            found = true;
                            break;
                        }
                    }
                }
                else
                {
                    found = true;
                }

                if found && o.0.eligible_to_spend(height, minimum_confirmations) {
                    count = count + 1;
                }
            }
            Ok(())
        })?;
        Ok(count)
    }

    pub fn get_outputs(&self) -> Result<HashMap<pedersen::Commitment, (Identifier, Option<u64>)>> {
        let wallet = self.get_wallet_instance()?;
        let mut outputs = HashMap::new();
        controller::owner_single_use(wallet.clone(), |api| {
            outputs = api.retrieve_map_wallet_outputs()?;
            Ok(())
        })?;

        Ok(outputs)
    }

    pub fn outputs(&self, show_spent: bool, pagination_start: u32, pagination_length: u32) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        let result = controller::owner_single_use(wallet.clone(), |api| {
            let (height, _) = api.node_height()?;
            let (validated, outputs) = api.retrieve_outputs(show_spent, true, None, pagination_start, pagination_length)?;
            display::outputs(&self.active_account, height, validated, outputs, true)?;
            Ok(())
        })?;
        Ok(result)
    }

    pub fn initiate_send_tx(
        &self,
        address: Option<String>,
        amount: u64,
        minimum_confirmations: u64,
        selection_strategy: &str,
        change_outputs: usize,
        max_outputs: usize,
        message: Option<String>,
        outputs: Option<Vec<&str>>,
        version: Option<u16>,
        routputs: usize,
        height: Option<u64>,
        node_outputs: Option<Vec<Output>>,
    ) -> Result<Slate> {
        let wallet = self.get_wallet_instance()?;
        let mut s: Slate = Slate::blank(0);
        controller::owner_single_use(wallet.clone(), |api| {
            let (slate, lock_fn) = api.initiate_tx(
                address,
                amount,
                minimum_confirmations,
                max_outputs,
                change_outputs,
                selection_strategy == "all",
                message,
                outputs,
                version,
                routputs,
                height,
                node_outputs,
            )?;
            api.tx_lock_outputs(&slate.tx, lock_fn)?;
            s = slate;
            Ok(())
        })?;
        Ok(s)
    }

    pub fn initiate_receive_tx(&self, amount: u64, num_outputs: usize) -> Result<Slate> {
        let wallet = self.get_wallet_instance()?;
        let mut s: Slate = Slate::blank(0);
        controller::foreign_single_use(wallet.clone(), |api| {
            let (slate, add_fn) = api.initiate_receive_tx(amount, num_outputs, None)?;
            api.tx_add_invoice_outputs(&slate, add_fn)?;
            s = slate;
            Ok(())
        })?;
        Ok(s)
    }

    pub fn repost(&self, id: u32, fluff: bool) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| {
            let (_, txs) = api.retrieve_txs(true, Some(id), None)?;
            if txs.len() == 0 {
                return Err(ErrorKind::GenericError(format!(
                    "could not find transaction with id {}!",
                    id
                )))?;
            }
            let slate_id = txs[0].tx_slate_id;
            if let Some(slate_id) = slate_id {
                let stored_tx = api.get_stored_tx(&slate_id.to_string())?;
                api.post_tx(&stored_tx, fluff)?;
                Ok(())
            } else {
                Err(ErrorKind::GenericError(format!(
                    "no transaction data stored for id {}, can not repost!",
                    id
                )))?
            }
        })?;
        Ok(())
    }

    pub fn cancel(&self,
                  id: u32,
                  height: Option<u64>,
                  accumulator: Option<Vec<Output>>,
    ) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| api.cancel_tx(Some(id), None, height, accumulator))?;
        Ok(())
    }

    pub fn restore_state(&self) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| api.restore())?;
        Ok(())
    }

    pub fn check_repair(&self) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| api.check_repair())?;
        Ok(())
    }

    pub fn build_coinbase(&self, block_fees: &BlockFees) -> Result<CbData> {
        let wallet = self.get_wallet_instance()?;
        let mut cb_data = None;
        controller::foreign_single_use(wallet.clone(), |api| {
            cb_data = Some(api.build_coinbase(block_fees)?);
            Ok(())
        })?;
        Ok(cb_data.unwrap())
    }

    pub fn process_sender_initiated_slate(
        &self,
        address: Option<String>,
        slate: &mut Slate,
        key_id: Option<&str>,
        output_amounts: Option<Vec<u64>>,
    ) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::foreign_single_use(wallet.clone(), |api| {
            api.receive_tx(address, slate, None, key_id, output_amounts)?;
            Ok(())
        })
        .map_err(|_| ErrorKind::GrinWalletReceiveError)?;
        Ok(())
    }

    pub fn process_receiver_initiated_slate(&self, slate: &mut Slate) -> Result<()> {
        // reject by default unless wallet is set to auto accept invoices under a certain threshold
        let max_auto_accept_invoice = self
            .max_auto_accept_invoice
            .ok_or(ErrorKind::DoesNotAcceptInvoices)?;

        if slate.amount > max_auto_accept_invoice {
            Err(ErrorKind::InvoiceAmountTooBig(slate.amount))?;
        }

        let wallet = self.get_wallet_instance()?;

        controller::owner_single_use(wallet.clone(), |api| {
            let lock_fn = api.invoice_tx(slate, 10, 500, 1, false, None)?;
            api.tx_lock_outputs(&slate.tx, lock_fn)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn submit(&self, txn: &mut Transaction) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| {
            api.post_tx(&txn, false)?;
            Ok(())
        })
        .map_err(|_| ErrorKind::GrinWalletPostError)?;

        Ok(())
    }

    pub fn finalize_slate(&self, slate: &mut Slate, tx_proof: Option<&mut TxProof>) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        let mut should_post: bool = false;
        controller::owner_single_use(wallet.clone(), |api| {
            api.verify_slate_messages(&slate)?;
            Ok(())
        })
        .map_err(|_| ErrorKind::GrinWalletVerifySlateMessagesError)?;
        controller::owner_single_use(wallet.clone(), |api| {
            should_post = api.finalize_tx(slate, tx_proof)?;
            Ok(())
        })
        .map_err(|_| ErrorKind::GrinWalletFinalizeError)?;
        if should_post {
            controller::owner_single_use(wallet.clone(), |api| {
                api.post_tx(&slate.tx, false)?;
                Ok(())
            })
            .map_err(|_| ErrorKind::GrinWalletPostError)?;
        }
        Ok(())
    }

    pub fn retrieve_summary_info(&self, refresh: bool, height: Option<u64>, accumulator: Option<Vec<Output>>) -> Result<WalletInfo> {
        let wallet = self.get_wallet_instance()?;
        let mut info = None;
        controller::owner_single_use(wallet.clone(), |api| {
            let (_, i) = api.retrieve_summary_info(refresh, 10, height, accumulator)?;
            info = Some(i);
            Ok(())
        })?;
        Ok(info.unwrap())
    }

    pub fn retrieve_outputs(
        &self,
        include_spent: bool,
        refresh_from_node: bool,
        tx_id: Option<u32>,
    ) -> Result<(bool, Vec<(OutputData, pedersen::Commitment)>)> {
        let wallet = self.get_wallet_instance()?;
        let mut result = (false, vec![]);
        controller::owner_single_use(wallet.clone(), |api| {
            result = api.retrieve_outputs(include_spent, refresh_from_node, tx_id, 0, 0)?;
            Ok(())
        })?;
        Ok(result)
    }

    pub fn retrieve_txs(
        &self,
        refresh_from_node: bool,
        tx_id: Option<u32>,
        tx_slate_id: Option<Uuid>,
    ) -> Result<(bool, Vec<TxLogEntry>)> {
        let wallet = self.get_wallet_instance()?;
        let mut result = (false, vec![]);
        controller::owner_single_use(wallet.clone(), |api| {
            result = api.retrieve_txs(refresh_from_node, tx_id, tx_slate_id)?;
            Ok(())
        })?;
        Ok(result)
    }

    pub fn get_stored_tx(&self, uuid: &str) -> Result<Transaction> {
        let wallet = self.get_wallet_instance()?;
        let mut result = Transaction::default();
        controller::owner_single_use(wallet.clone(), |api| {
            result = api.get_stored_tx(uuid)?;
            Ok(())
        })?;
        Ok(result)
    }

    pub fn post_tx(&self, tx: &Transaction, fluff: bool) -> Result<()> {
        let wallet = self.get_wallet_instance()?;
        controller::owner_single_use(wallet.clone(), |api| {
            api.post_tx(tx, fluff)?;
            Ok(())
        })?;
        Ok(())
    }

    pub fn _node_height(&self) -> Result<(u64, bool)> {
        let wallet = self.get_wallet_instance()?;
        let mut result = (0, false);
        controller::owner_single_use(wallet.clone(), |api| {
            result = api.node_height()?;
            Ok(())
        })?;
        Ok(result)
    }

    pub fn derive_address_key(&self, index: u32) -> Result<SecretKey> {
        let wallet = self.get_wallet_instance()?;
        let mut w = wallet.lock();
        w.open_with_credentials()?;
        derive_address_key(w.keychain(), index).map_err(|e| e.into())
    }

    pub fn get_tx_proof(&self, id: u32) -> Result<TxProof> {
        let wallet = self.get_wallet_instance()?;
        let mut api = Wallet713OwnerAPI::new(wallet.clone());
        api.get_stored_tx_proof(id)
    }

    pub fn verify_tx_proof(
        &self,
        tx_proof: &TxProof,
    ) -> Result<(Option<String>, String, u64, Vec<String>, String)> {
        let wallet = self.get_wallet_instance()?;
        let mut api = Wallet713OwnerAPI::new(wallet.clone());
        let (sender, receiver, amount, outputs, excess_sum) = api.verify_tx_proof(tx_proof)?;

        let outputs = outputs
            .iter()
            .map(|o| grin_util::to_hex(o.0.to_vec()))
            .collect();

        Ok((
            sender.map(|a| a.public_key.clone()),
            receiver.public_key.clone(),
            amount,
            outputs,
            excess_sum.to_hex(),
        ))
    }

    fn init_seed(
        &self,
        wallet_config: &WalletConfig,
        passphrase: &str,
        create_new: bool,
        create_file: bool,
        seed: Option<WalletSeed>,
    ) -> Result<WalletSeed> {
        let result = WalletSeed::from_file(&wallet_config.data_file_dir, passphrase);
        let seed = match result {
            Ok(seed) => seed,
            Err(_) => {
                // could not load from file, let's create a new one
                if create_new {
                    WalletSeed::init_file_impl(&wallet_config.data_file_dir, 32, None, passphrase, create_file,!create_file, seed)?
                } else {
                    return Err(ErrorKind::WalletSeedCouldNotBeOpened.into());
                }
            }
        };
        Ok(seed)
    }

    pub fn get_wallet_instance(
        &self,
    ) -> Result<Arc<Mutex<dyn WalletInst<impl NodeClient + 'static, ExtKeychain>>>> {
        if let Some(ref backend) = self.backend {
            Ok(backend.clone())
        } else {
            Err(ErrorKind::NoWallet)?
        }
    }

    fn create_wallet_instance(
        &mut self,
        config: &Wallet713Config,
        account: &str,
        passphrase: &str,
    ) -> Result<()> {
        let wallet_config = config.as_wallet_config()?;
        let node_client = HTTPNodeClient::new(
            &wallet_config.check_node_api_http_addr,
            config.mwc_node_secret().clone(),
        );
        let _ = WalletSeed::from_file(&wallet_config.data_file_dir, passphrase)?;
        let mut db_wallet = Backend::new(&wallet_config, passphrase, node_client)?;
        db_wallet.set_parent_key_id_by_name(account)?;
        self.backend = Some(Arc::new(Mutex::new(db_wallet)));
        Ok(())
    }

    fn init_backend(
        &self,
        wallet_config: &WalletConfig,
        wallet713_config: &Wallet713Config,
        passphrase: &str,
    ) -> Result<Backend<HTTPNodeClient, ExtKeychain>> {
        let node_api_client = HTTPNodeClient::new(
            &wallet_config.check_node_api_http_addr,
            wallet713_config.mwc_node_secret().clone(),
        );
        let backend = Backend::new(wallet_config, passphrase, node_api_client)?;
        Ok(backend)
    }
}
