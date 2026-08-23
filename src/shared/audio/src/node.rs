use crate::{AudioSample, Result};

/// Trait for audio processing nodes in a linked-list style chain
pub trait AudioNode: Send {
    /// Process audio samples and optionally pass to next node
    /// Returns true if processing was successful, false if node should be bypassed
    fn process(&mut self, sample: &mut AudioSample) -> Result<bool>;

    /// Reset the node state (useful for filters, delays, etc.)
    fn reset(&mut self);

    /// Get node name for debugging
    fn name(&self) -> &str;

    /// Check if node is enabled
    fn is_enabled(&self) -> bool;

    /// Enable/disable node
    fn set_enabled(&mut self, enabled: bool);

    /// Set the next node in the chain
    fn set_next(&mut self, next: Box<dyn AudioNode>);

    /// Process through entire node chain starting from this node
    fn process_chain(&mut self, sample: &mut AudioSample) -> Result<()>;

    /// Check if this node has a next node
    fn has_next(&self) -> bool;

    /// Convert this node into a chain (all nodes are chainable)
    fn into_chain(self) -> AudioNodeChain
    where
        Self: Sized + 'static,
    {
        AudioNodeChain::new(Box::new(self))
    }
}

/// Base implementation for audio processing nodes with automatic chaining
pub struct AudioNodeChain {
    /// The current node
    node: Box<dyn AudioNode>,
    /// Optional next node in the chain
    next: Option<Box<dyn AudioNode>>,
}

impl AudioNodeChain {
    /// Create a new node chain starting with the given node
    pub fn new(node: Box<dyn AudioNode>) -> Self {
        Self { node, next: None }
    }

    /// Chain another node to this one
    pub fn chain(mut self, next_node: Box<dyn AudioNode>) -> Self {
        if let Some(ref mut next) = self.next {
            // If we already have a next node, chain to the end
            next.set_next(next_node);
        } else {
            // This is the first next node
            self.next = Some(next_node);
        }
        self
    }

    /// Process audio through the entire chain
    pub fn process(&mut self, sample: &mut AudioSample) -> Result<()> {
        // Process through current node
        if self.node.is_enabled() {
            match self.node.process(sample) {
                Ok(true) => {
                    log::debug!("Processed with {}", self.node.name());
                }
                Ok(false) => {
                    log::debug!("Bypassed {}", self.node.name());
                }
                Err(e) => {
                    log::warn!("Error in node {}: {}", self.node.name(), e);
                    // Continue with other nodes instead of failing completely
                }
            }
        }

        // Process through next node if it exists
        if let Some(ref mut next) = self.next {
            next.process_chain(sample)?;
        }

        Ok(())
    }

    /// Reset all nodes in the chain
    pub fn reset(&mut self) {
        self.node.reset();
        if let Some(ref mut next) = self.next {
            next.reset();
        }
    }

    /// Get the length of the node chain
    pub fn len(&self) -> usize {
        1 + if let Some(ref next) = self.next {
            if next.has_next() {
                1
            } else {
                0
            } // Simplified for now
        } else {
            0
        }
    }

    /// Check if chain is empty (should always be false since we have at least one node)
    pub fn is_empty(&self) -> bool {
        false
    }
}

/// Base struct for implementing audio nodes
pub struct BaseAudioNode {
    name: String,
    enabled: bool,
    next: Option<Box<dyn AudioNode>>,
}

impl BaseAudioNode {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            enabled: true,
            next: None,
        }
    }

    pub fn process_next(&mut self, sample: &mut AudioSample) -> Result<()> {
        if let Some(ref mut next) = self.next {
            next.process_chain(sample)?;
        }
        Ok(())
    }
}

impl AudioNode for BaseAudioNode {
    fn process(&mut self, _sample: &mut AudioSample) -> Result<bool> {
        // Base implementation does nothing
        Ok(true)
    }

    fn reset(&mut self) {
        if let Some(ref mut next) = self.next {
            next.reset();
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn set_next(&mut self, next: Box<dyn AudioNode>) {
        self.next = Some(next);
    }

    fn process_chain(&mut self, sample: &mut AudioSample) -> Result<()> {
        // This is just the base implementation - actual nodes should override this
        self.process_next(sample)
    }

    fn has_next(&self) -> bool {
        self.next.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test node that doubles all samples
    struct TestDoubleNode {
        base: BaseAudioNode,
    }

    impl TestDoubleNode {
        fn new() -> Self {
            Self {
                base: BaseAudioNode::new("TestDoubleNode"),
            }
        }
    }

    impl AudioNode for TestDoubleNode {
        fn process(&mut self, sample: &mut AudioSample) -> Result<bool> {
            for value in &mut sample.data {
                *value *= 2.0;
            }
            Ok(true)
        }

        fn reset(&mut self) {
            self.base.reset();
        }

        fn name(&self) -> &str {
            self.base.name()
        }

        fn is_enabled(&self) -> bool {
            self.base.is_enabled()
        }

        fn set_enabled(&mut self, enabled: bool) {
            self.base.set_enabled(enabled);
        }

        fn set_next(&mut self, next: Box<dyn AudioNode>) {
            self.base.set_next(next);
        }

        fn process_chain(&mut self, sample: &mut AudioSample) -> Result<()> {
            // Process through this node first
            if self.is_enabled() {
                self.process(sample)?;
            }
            // Then process through next node
            self.base.process_next(sample)
        }

        fn has_next(&self) -> bool {
            self.base.has_next()
        }
    }

    // Test node that adds 0.1 to all samples
    struct TestAddNode {
        base: BaseAudioNode,
    }

    impl TestAddNode {
        fn new() -> Self {
            Self {
                base: BaseAudioNode::new("TestAddNode"),
            }
        }
    }

    impl AudioNode for TestAddNode {
        fn process(&mut self, sample: &mut AudioSample) -> Result<bool> {
            for value in &mut sample.data {
                *value += 0.1;
            }
            Ok(true)
        }

        fn reset(&mut self) {
            self.base.reset();
        }

        fn name(&self) -> &str {
            self.base.name()
        }

        fn is_enabled(&self) -> bool {
            self.base.is_enabled()
        }

        fn set_enabled(&mut self, enabled: bool) {
            self.base.set_enabled(enabled);
        }

        fn set_next(&mut self, next: Box<dyn AudioNode>) {
            self.base.set_next(next);
        }

        fn process_chain(&mut self, sample: &mut AudioSample) -> Result<()> {
            // Process through this node first
            if self.is_enabled() {
                self.process(sample)?;
            }
            // Then process through next node
            self.base.process_next(sample)
        }

        fn has_next(&self) -> bool {
            self.base.has_next()
        }
    }

    #[test]
    fn test_single_node() {
        let mut node_chain = TestDoubleNode::new().into_chain();

        let mut sample = AudioSample {
            data: vec![0.1, 0.2, 0.3, 0.4],
            channels: 2,
            sample_rate: 44100,
            frames: 2,
        };

        node_chain.process(&mut sample).unwrap();

        // Values should be doubled
        assert_eq!(sample.data, vec![0.2, 0.4, 0.6, 0.8]);
    }

    #[test]
    fn test_chained_nodes() {
        let mut node_chain = TestDoubleNode::new()
            .into_chain()
            .chain(Box::new(TestAddNode::new()));

        let mut sample = AudioSample {
            data: vec![0.1, 0.2],
            channels: 1,
            sample_rate: 44100,
            frames: 2,
        };

        node_chain.process(&mut sample).unwrap();

        // First doubled (0.1 -> 0.2, 0.2 -> 0.4), then add 0.1 (0.2 -> 0.3, 0.4 -> 0.5)
        assert!((sample.data[0] - 0.3).abs() < 0.001);
        assert!((sample.data[1] - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_node_enable_disable() {
        let mut double_node = TestDoubleNode::new();
        double_node.set_enabled(false);

        let mut node_chain = double_node.into_chain();

        let mut sample = AudioSample {
            data: vec![0.1, 0.2, 0.3, 0.4],
            channels: 2,
            sample_rate: 44100,
            frames: 2,
        };

        let original_data = sample.data.clone();
        node_chain.process(&mut sample).unwrap();

        // Values should be unchanged (node disabled)
        assert_eq!(sample.data, original_data);
    }
}
