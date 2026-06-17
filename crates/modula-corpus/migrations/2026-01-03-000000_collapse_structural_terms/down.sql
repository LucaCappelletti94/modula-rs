ALTER TABLE analyses DROP COLUMN cohesion_term;
ALTER TABLE analyses ADD COLUMN modularity_term DOUBLE;
ALTER TABLE analyses ADD COLUMN divergence_term DOUBLE;
ALTER TABLE analyses ADD COLUMN headline_depth_averaged DOUBLE;
