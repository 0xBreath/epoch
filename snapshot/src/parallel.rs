use crate::archive::{AppendVecIterator, AppendVecMeta};
use crossbeam::sync::WaitGroup;

pub trait AppendVecConsumerFactory {
    type Consumer: AppendVecConsumer + Send + 'static;
    fn new_consumer(&mut self) -> anyhow::Result<Self::Consumer>;
}

pub trait AppendVecConsumer {
    fn on_append_vec(&mut self, append_vec: AppendVecMeta) -> anyhow::Result<()>;
}

pub fn par_iter_append_vecs<A>(
    iterator: AppendVecIterator<'_>,
    consumers: &mut A,
    num_threads: usize,
) -> anyhow::Result<()>
where
    A: AppendVecConsumerFactory,
{
    let (tx, rx) = crossbeam::channel::bounded::<AppendVecMeta>(num_threads);

    let wg = WaitGroup::new();
    let mut consumer_vec = Vec::with_capacity(num_threads);
    for _ in 0..num_threads {
        consumer_vec.push(consumers.new_consumer()?);
    }

    for mut consumer in consumer_vec {
        let rx = rx.clone();
        let wg = wg.clone();
        std::thread::spawn(move || {
            while let Ok(item) = rx.recv() {
                consumer.on_append_vec(item).expect("insert failed")
            }
            drop(wg);
        });
    }

    for append_vec in iterator {
        let append_vec = append_vec?;
        tx.send(append_vec).expect("failed to send AppendVec");
    }
    drop(tx);
    wg.wait();
    Ok(())
}
